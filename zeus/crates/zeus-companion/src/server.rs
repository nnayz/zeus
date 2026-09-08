//! Bounded HTTP/1 + WebSocket sidecar. All session authority stays over IPC.
use crate::{
    auth::{AuthStore, now_ms, random_token},
    config::{Config, invalid, secure_read},
};
use axum::{
    Router,
    body::to_bytes,
    extract::{
        Path, Query, Request, State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    http::{HeaderValue, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::{Serialize, de::DeserializeOwned};
use std::{
    collections::{HashMap, VecDeque},
    io,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::{
    net::TcpListener,
    sync::{Semaphore, broadcast, watch},
    task::JoinSet,
};
use zeus_client::{ClientError, ConnectionState, DaemonClient};
use zeus_companion_api::*;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(8);
const WRITE_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_CONNECTIONS: usize = 32;
const MAX_WEBSOCKETS: usize = 8;

struct ShutdownOnDrop(watch::Sender<bool>);
impl Drop for ShutdownOnDrop {
    fn drop(&mut self) {
        self.0.send_replace(true);
    }
}

#[derive(Clone)]
struct App {
    client: Arc<DaemonClient>,
    auth: AuthStore,
    config: Config,
    events: Arc<Mutex<EventHub>>,
    rate: Arc<Mutex<Rate>>,
    ws: Arc<Semaphore>,
    shutdown: watch::Receiver<bool>,
}
struct Rate {
    since: Instant,
    count: u32,
    devices: HashMap<String, u32>,
}
impl Rate {
    fn admit(&mut self, device: Option<&str>) -> bool {
        if self.since.elapsed() >= Duration::from_secs(60) {
            self.since = Instant::now();
            self.count = 0;
            self.devices.clear();
        }
        if let Some(device) = device {
            if !self.devices.contains_key(device) && self.devices.len() >= 64 {
                return false;
            }
            let n = self.devices.entry(device.to_owned()).or_default();
            *n += 1;
            *n <= 240
        } else {
            self.count += 1;
            self.count <= 600
        }
    }
}
pub struct EventHub {
    stream_id: String,
    sequence: u64,
    ring: VecDeque<Event>,
    sender: broadcast::Sender<Event>,
}
impl EventHub {
    pub fn new() -> io::Result<Self> {
        Ok(Self {
            stream_id: random_token()?,
            sequence: 0,
            ring: VecDeque::new(),
            sender: broadcast::channel(16).0,
        })
    }
    pub fn publish(&mut self, kind: &str) {
        self.sequence += 1;
        let event = Event {
            cursor: self.cursor(),
            kind: kind.into(),
        };
        if self.ring.len() == EVENT_WINDOW {
            self.ring.pop_front();
        }
        self.ring.push_back(event.clone());
        let _ = self.sender.send(event);
    }
    fn cursor(&self) -> Cursor {
        Cursor {
            stream_id: self.stream_id.clone(),
            sequence: self.sequence,
        }
    }
    pub fn replay(&self, cursor: Option<&Cursor>) -> Vec<Event> {
        if let Some(cursor) = cursor {
            let first = self
                .ring
                .front()
                .map_or(self.sequence + 1, |e| e.cursor.sequence);
            if cursor.stream_id == self.stream_id
                && cursor.sequence <= self.sequence
                && cursor.sequence.saturating_add(1) >= first
            {
                return self
                    .ring
                    .iter()
                    .filter(|e| e.cursor.sequence > cursor.sequence)
                    .cloned()
                    .collect();
            }
        }
        vec![Event {
            cursor: self.cursor(),
            kind: "resync_required".into(),
        }]
    }
}

#[derive(Debug)]
struct Failure(StatusCode, &'static str);
impl IntoResponse for Failure {
    fn into_response(self) -> Response {
        reply(
            self.0,
            &ApiError {
                code: self.1.into(),
            },
        )
    }
}
fn reply<T: Serialize>(status: StatusCode, value: &T) -> Response {
    match serde_json::to_vec(value) {
        Ok(bytes) if bytes.len() <= MAX_RESPONSE => (
            status,
            [
                (header::CONTENT_TYPE, "application/json"),
                (header::CACHE_CONTROL, "no-store"),
            ],
            bytes,
        )
            .into_response(),
        _ => (
            StatusCode::BAD_GATEWAY,
            [(header::CONTENT_TYPE, "application/json")],
            "{\"code\":\"projection_limit\"}",
        )
            .into_response(),
    }
}
fn failure(code: &'static str) -> Failure {
    Failure(StatusCode::BAD_REQUEST, code)
}
fn engine_error(error: ClientError) -> Failure {
    match error {
        ClientError::Control(e) => match e.code.as_str() {
            "not_found" => Failure(StatusCode::NOT_FOUND, "not_found"),
            "stale_engine" => Failure(StatusCode::CONFLICT, "stale_engine"),
            "stale_revision" => Failure(StatusCode::CONFLICT, "stale_revision"),
            "replayed_mutation" => Failure(StatusCode::CONFLICT, "replayed_mutation"),
            "mutation_conflict" => Failure(StatusCode::CONFLICT, "mutation_conflict"),
            "confirmation_required" => failure("confirmation_required"),
            "busy" | "mutation_window_full" => Failure(StatusCode::TOO_MANY_REQUESTS, "busy"),
            "capability_unavailable" => {
                Failure(StatusCode::NOT_IMPLEMENTED, "capability_unavailable")
            }
            "stale_controller_epoch" => Failure(StatusCode::CONFLICT, "stale_controller_epoch"),
            "not_controller" => Failure(StatusCode::CONFLICT, "not_controller"),
            "controller_busy" => Failure(StatusCode::CONFLICT, "controller_busy"),
            "command_sequence" => Failure(StatusCode::CONFLICT, "command_sequence"),
            "input_unconfirmed" => Failure(StatusCode::CONFLICT, "input_unconfirmed"),
            "outcome_unknown" => Failure(StatusCode::CONFLICT, "outcome_unknown"),
            "terminal_geometry" => Failure(StatusCode::UNPROCESSABLE_ENTITY, "terminal_geometry"),
            _ => Failure(StatusCode::CONFLICT, "engine_rejected"),
        },
        ClientError::Timeout(_) => Failure(StatusCode::GATEWAY_TIMEOUT, "outcome_unknown"),
        _ => Failure(StatusCode::SERVICE_UNAVAILABLE, "engine_unavailable"),
    }
}
fn credential(headers: &axum::http::HeaderMap) -> Result<&str, Failure> {
    if headers.get_all(header::AUTHORIZATION).iter().count() != 1 {
        return Err(Failure(StatusCode::UNAUTHORIZED, "unauthorized"));
    }
    headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .filter(|s| s.len() == 64)
        .ok_or(Failure(StatusCode::UNAUTHORIZED, "unauthorized"))
}
fn authenticate(
    app: &App,
    headers: &axum::http::HeaderMap,
    scope: Scope,
) -> Result<Device, Failure> {
    let device = app
        .auth
        .authenticate(credential(headers)?, scope, now_ms())
        .map_err(|_| Failure(StatusCode::UNAUTHORIZED, "unauthorized"))?;
    if !app.rate.lock().unwrap().admit(Some(&device.id)) {
        return Err(Failure(StatusCode::TOO_MANY_REQUESTS, "rate_limited"));
    }
    Ok(device)
}
async fn decode<T: DeserializeOwned>(request: Request, limit: usize) -> Result<T, Failure> {
    if request
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_none_or(|v| v != "application/json")
    {
        return Err(Failure(StatusCode::UNSUPPORTED_MEDIA_TYPE, "json_required"));
    }
    let bytes = to_bytes(request.into_body(), limit)
        .await
        .map_err(|_| Failure(StatusCode::PAYLOAD_TOO_LARGE, "body_limit"))?;
    serde_json::from_slice(&bytes).map_err(|_| failure("invalid_request"))
}
async fn protect(State(app): State<App>, request: Request, next: Next) -> Response {
    let origin = request.headers().get(header::ORIGIN).cloned();
    if let Some(origin) = &origin
        && (request.headers().get_all(header::ORIGIN).iter().count() != 1
            || !origin
                .to_str()
                .is_ok_and(|o| app.config.origins.iter().any(|allowed| allowed == o)))
    {
        return Failure(StatusCode::FORBIDDEN, "origin_denied").into_response();
    }
    if request.uri().path() != "/v1/events" && request.headers().contains_key(header::UPGRADE) {
        return failure("upgrade_denied").into_response();
    }
    if !app.rate.lock().unwrap().admit(None) {
        return Failure(StatusCode::TOO_MANY_REQUESTS, "rate_limited").into_response();
    }
    let mut response = if request.method() == axum::http::Method::OPTIONS {
        if origin.is_none() {
            return Failure(StatusCode::FORBIDDEN, "origin_required").into_response();
        }
        StatusCode::NO_CONTENT.into_response()
    } else {
        match tokio::time::timeout(REQUEST_TIMEOUT, next.run(request)).await {
            Ok(response) => response,
            Err(_) => Failure(StatusCode::GATEWAY_TIMEOUT, "outcome_unknown").into_response(),
        }
    };
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response.headers_mut().insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    if let Some(origin) = origin {
        response
            .headers_mut()
            .insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin);
        response
            .headers_mut()
            .insert(header::VARY, HeaderValue::from_static("Origin"));
        response.headers_mut().insert(
            header::ACCESS_CONTROL_ALLOW_METHODS,
            HeaderValue::from_static("GET, POST, OPTIONS"),
        );
        response.headers_mut().insert(
            header::ACCESS_CONTROL_ALLOW_HEADERS,
            HeaderValue::from_static("Authorization, Content-Type"),
        );
    }
    response
}
async fn hello(
    State(app): State<App>,
    headers: axum::http::HeaderMap,
) -> Result<Response, Failure> {
    authenticate(&app, &headers, Scope::Read)?;
    let mut hello = app.client.companion_hello().await.map_err(engine_error)?;
    hello.server_id = app
        .auth
        .server_id()
        .map_err(|_| failure("auth_unavailable"))?;
    Ok(reply(StatusCode::OK, &hello))
}
async fn pair(State(app): State<App>, request: Request) -> Result<Response, Failure> {
    let p: PairRequest = decode(request, 1024).await?;
    if p.api_major != API_MAJOR {
        return Err(Failure(StatusCode::UPGRADE_REQUIRED, "version_mismatch"));
    }
    let result = app
        .auth
        .pair(&p.code, &p.device_name, &p.expected_server_id, now_ms())
        .map_err(|_| Failure(StatusCode::UNAUTHORIZED, "pairing_denied"))?;
    Ok(reply(StatusCode::OK, &result))
}
async fn sessions(
    State(app): State<App>,
    Query(page): Query<PageRequest>,
    headers: axum::http::HeaderMap,
) -> Result<Response, Failure> {
    authenticate(&app, &headers, Scope::Read)?;
    Ok(reply(
        StatusCode::OK,
        &app.client
            .companion_sessions(&page)
            .await
            .map_err(engine_error)?,
    ))
}
async fn projects(
    State(app): State<App>,
    Query(page): Query<PageRequest>,
    headers: axum::http::HeaderMap,
) -> Result<Response, Failure> {
    authenticate(&app, &headers, Scope::Read)?;
    Ok(reply(
        StatusCode::OK,
        &app.client
            .companion_projects(&page)
            .await
            .map_err(engine_error)?,
    ))
}
async fn session(
    State(app): State<App>,
    Path(id): Path<String>,
    headers: axum::http::HeaderMap,
) -> Result<Response, Failure> {
    authenticate(&app, &headers, Scope::Read)?;
    if !valid_id(&id) {
        return Err(failure("invalid_request"));
    }
    Ok(reply(
        StatusCode::OK,
        &app.client
            .companion_session(&id)
            .await
            .map_err(engine_error)?,
    ))
}
async fn mutate(
    State(app): State<App>,
    Path(id): Path<String>,
    request: Request,
) -> Result<Response, Failure> {
    authenticate(&app, request.headers(), Scope::Lifecycle)?;
    let token = zeroize::Zeroizing::new(credential(request.headers())?.to_owned());
    if !valid_id(&id) {
        return Err(failure("invalid_request"));
    }
    let mutation: Mutation = decode(request, 4096).await?;
    let device = app
        .auth
        .authenticate(&token, Scope::Lifecycle, now_ms())
        .map_err(|_| Failure(StatusCode::UNAUTHORIZED, "unauthorized"))?;
    Ok(reply(
        StatusCode::OK,
        &app.client
            .companion_mutate(&id, &device.id, &mutation)
            .await
            .map_err(engine_error)?,
    ))
}

fn internal_epoch(epoch: ControlEpoch) -> zeus_proto::terminal::ControlEpoch {
    zeus_proto::terminal::ControlEpoch {
        incarnation: epoch.incarnation,
        generation: epoch.generation,
    }
}
fn external_control(state: zeus_proto::terminal::ControlState) -> ControlState {
    ControlState {
        epoch: ControlEpoch {
            incarnation: state.epoch.incarnation,
            generation: state.epoch.generation,
        },
        command_seq: state.command_seq,
        owner: state.owner.map(|owner| Controller {
            id: owner.id,
            label: owner.label,
            role: match owner.role {
                zeus_proto::ClientRole::Desktop => "desktop",
                zeus_proto::ClientRole::Mobile => "mobile",
                _ => "unknown",
            }
            .into(),
        }),
    }
}
async fn screen(
    State(app): State<App>,
    Path(id): Path<String>,
    headers: axum::http::HeaderMap,
) -> Result<Response, Failure> {
    authenticate(&app, &headers, Scope::Read)?;
    if !valid_id(&id) {
        return Err(failure("invalid_request"));
    }
    let snapshot = app
        .client
        .terminal_snapshot(&zeus_proto::terminal::TerminalSnapshotParams {
            session_id: zeus_proto::SessionId(id.clone()),
            protocol: 1,
            since: None,
        })
        .await
        .map_err(engine_error)?;
    let grid = snapshot
        .decode_grid()
        .map_err(|_| Failure(StatusCode::BAD_GATEWAY, "invalid_snapshot"))?
        .ok_or(Failure(StatusCode::BAD_GATEWAY, "snapshot_required"))?;
    let mut text = String::new();
    for (index, row) in grid.changed_rows.iter().enumerate() {
        if index > 0 {
            text.push('\n');
        }
        for cell in &row.cells {
            let ch = char::from_u32(cell.scalar)
                .filter(|c| !c.is_control())
                .unwrap_or(' ');
            text.push(ch);
        }
    }
    app.auth
        .authenticate(credential(&headers)?, Scope::Read, now_ms())
        .map_err(|_| Failure(StatusCode::UNAUTHORIZED, "unauthorized"))?;
    let truncated = text.len() > 128 * 1024;
    Ok(reply(
        StatusCode::OK,
        &Screen {
            session_id: id,
            incarnation: snapshot.cursor.incarnation,
            screen_sequence: snapshot.cursor.sequence,
            text: bounded_string(&text, 128 * 1024),
            cols: grid.cols,
            rows: grid.rows,
            cursor_row: grid.cursor_row,
            cursor_col: grid.cursor_col,
            control: external_control(snapshot.control),
            exited: snapshot.exited,
            truncated,
        },
    ))
}
async fn acquire(
    State(app): State<App>,
    Path(id): Path<String>,
    request: Request,
) -> Result<Response, Failure> {
    authenticate(&app, request.headers(), Scope::Interact)?;
    let token = zeroize::Zeroizing::new(credential(request.headers())?.to_owned());
    if !valid_id(&id) {
        return Err(failure("invalid_request"));
    }
    let p: AcquireControl = decode(request, 1024).await?;
    let device = app
        .auth
        .authenticate(&token, Scope::Interact, now_ms())
        .map_err(|_| Failure(StatusCode::UNAUTHORIZED, "unauthorized"))?;
    let state = app
        .client
        .acquire_terminal_control(&zeus_proto::terminal::AcquireControlParams {
            session_id: zeus_proto::SessionId(id),
            expected: internal_epoch(p.expected),
            owner: zeus_proto::terminal::Controller {
                id: device.id,
                label: device.name,
                role: zeus_proto::ClientRole::Mobile,
            },
            takeover: p.takeover,
        })
        .await
        .map_err(engine_error)?;
    Ok(reply(StatusCode::OK, &external_control(state)))
}
async fn release(
    State(app): State<App>,
    Path(id): Path<String>,
    request: Request,
) -> Result<Response, Failure> {
    authenticate(&app, request.headers(), Scope::Interact)?;
    let token = zeroize::Zeroizing::new(credential(request.headers())?.to_owned());
    if !valid_id(&id) {
        return Err(failure("invalid_request"));
    }
    let p: ReleaseControl = decode(request, 1024).await?;
    let device = app
        .auth
        .authenticate(&token, Scope::Interact, now_ms())
        .map_err(|_| Failure(StatusCode::UNAUTHORIZED, "unauthorized"))?;
    let state = app
        .client
        .release_terminal_control(&zeus_proto::terminal::ReleaseControlParams {
            session_id: zeus_proto::SessionId(id),
            expected: internal_epoch(p.expected),
            owner_id: device.id,
        })
        .await
        .map_err(engine_error)?;
    Ok(reply(StatusCode::OK, &external_control(state)))
}
async fn send_text(
    State(app): State<App>,
    Path(id): Path<String>,
    request: Request,
) -> Result<Response, Failure> {
    authenticate(&app, request.headers(), Scope::Interact)?;
    let token = zeroize::Zeroizing::new(credential(request.headers())?.to_owned());
    if !valid_id(&id) {
        return Err(failure("invalid_request"));
    }
    let p: SendText = decode(request, MAX_BODY).await?;
    if p.text.len() > MAX_TEXT
        || p.text
            .chars()
            .any(|c| c.is_control() && c != '\n' && c != '\t')
    {
        return Err(failure("invalid_text"));
    }
    let device = app
        .auth
        .authenticate(&token, Scope::Interact, now_ms())
        .map_err(|_| Failure(StatusCode::UNAUTHORIZED, "unauthorized"))?;
    let state = app
        .client
        .terminal_send_text(&zeus_proto::terminal::TerminalSendTextParams {
            session_id: zeus_proto::SessionId(id),
            expected: internal_epoch(p.expected),
            owner_id: device.id,
            command_seq: p.command_seq,
            text: p.text,
            submit: p.submit,
        })
        .await
        .map_err(engine_error)?;
    Ok(reply(StatusCode::OK, &external_control(state)))
}
async fn revoke(
    State(app): State<App>,
    Path(id): Path<String>,
    request: Request,
) -> Result<Response, Failure> {
    let device = authenticate(&app, request.headers(), Scope::Read)?;
    let token = zeroize::Zeroizing::new(credential(request.headers())?.to_owned());
    if device.id != id {
        return Err(Failure(StatusCode::FORBIDDEN, "own_device_only"));
    }
    let _: std::collections::BTreeMap<String, serde_json::Value> = decode(request, 16).await?;
    app.auth
        .authenticate(&token, Scope::Read, now_ms())
        .map_err(|_| Failure(StatusCode::UNAUTHORIZED, "unauthorized"))?;
    app.auth
        .revoke(&id)
        .map_err(|_| failure("auth_unavailable"))?;
    Ok(reply(StatusCode::OK, &serde_json::json!({})))
}
async fn events(
    State(app): State<App>,
    axum::Extension(connection): axum::Extension<Arc<tokio::sync::OwnedSemaphorePermit>>,
    ws: WebSocketUpgrade,
) -> Result<Response, Failure> {
    let permit = app
        .ws
        .clone()
        .try_acquire_owned()
        .map_err(|_| Failure(StatusCode::TOO_MANY_REQUESTS, "connection_limit"))?;
    Ok(ws
        .max_message_size(2048)
        .max_frame_size(2048)
        .max_write_buffer_size(8192)
        .write_buffer_size(0)
        .on_upgrade(move |socket| async move {
            let _permit = permit;
            let _connection = connection;
            let mut shutdown = app.shutdown.clone();
            if *shutdown.borrow_and_update() {
                return;
            }
            tokio::select! {
                _=shutdown.changed()=>{},
                _=tokio::time::timeout(Duration::from_secs(300), stream_events(socket, app))=>{},
            }
        })
        .into_response())
}
async fn stream_events(mut socket: WebSocket, app: App) {
    let first = tokio::time::timeout(Duration::from_secs(5), socket.recv()).await;
    let Ok(Some(Ok(Message::Text(first)))) = first else {
        return;
    };
    let Ok(subscription) = serde_json::from_str::<Subscribe>(&first) else {
        return;
    };
    if subscription.api_major != API_MAJOR {
        return;
    }
    let token = zeroize::Zeroizing::new(subscription.token);
    let Ok(device) = app.auth.authenticate(&token, Scope::Read, now_ms()) else {
        return;
    };
    if !app.rate.lock().unwrap().admit(Some(&device.id)) {
        return;
    }
    let (mut receiver, replay) = {
        let hub = app.events.lock().unwrap();
        (
            hub.sender.subscribe(),
            hub.replay(subscription.cursor.as_ref()),
        )
    };
    let mut last = subscription.cursor.map_or(0, |c| c.sequence);
    for event in replay {
        if app
            .auth
            .authenticate(&token, Scope::Read, now_ms())
            .is_err()
        {
            return;
        }
        last = event.cursor.sequence;
        if !send_event(&mut socket, &event).await {
            return;
        }
    }
    loop {
        tokio::select! {
            received=receiver.recv()=>{
                if app.auth.authenticate(&token,Scope::Read,now_ms()).is_err(){return;}
                let event=match received {
                    Ok(event) if event.cursor.sequence>last=>event,
                    Ok(_)=>continue,
                    Err(broadcast::error::RecvError::Lagged(_))=>{let hub=app.events.lock().unwrap();Event{cursor:hub.cursor(),kind:"resync_required".into()}},
                    Err(_)=>return,
                };
                last=event.cursor.sequence;if !send_event(&mut socket,&event).await{return;}
            }
            incoming=socket.recv()=>{match incoming{Some(Ok(Message::Pong(_)))=>{},_=>return,}}
            _=tokio::time::sleep(Duration::from_secs(1))=>{if app.auth.authenticate(&token,Scope::Read,now_ms()).is_err(){return;}}
        }
    }
}
async fn send_event(socket: &mut WebSocket, event: &Event) -> bool {
    let Ok(json) = serde_json::to_string(event) else {
        return false;
    };
    tokio::time::timeout(WRITE_TIMEOUT, socket.send(Message::Text(json.into())))
        .await
        .is_ok_and(|r| r.is_ok())
}

pub async fn serve(
    listener: TcpListener,
    config: Config,
    auth: AuthStore,
    client: Arc<DaemonClient>,
    mut shutdown: watch::Receiver<bool>,
) -> io::Result<()> {
    config.validate()?;
    let actual = listener.local_addr()?;
    if actual.ip() != config.bind.ip()
        || (config.bind.port() != 0 && actual.port() != config.bind.port())
    {
        return Err(invalid("listener/config mismatch"));
    }
    let tls = match (&config.tls_certificate, &config.tls_key) {
        (Some(cert), Some(key)) => {
            use tokio_rustls::rustls;
            let cert = secure_read(cert, 64 * 1024)?;
            let key = zeroize::Zeroizing::new(secure_read(key, 16 * 1024)?);
            let certs =
                rustls_pemfile::certs(&mut cert.as_slice()).collect::<Result<Vec<_>, _>>()?;
            let key = rustls_pemfile::private_key(&mut key.as_slice())?
                .ok_or_else(|| invalid("missing TLS key"))?;
            let provider = Arc::new(rustls::crypto::ring::default_provider());
            let tls = rustls::ServerConfig::builder_with_provider(provider)
                .with_safe_default_protocol_versions()
                .map_err(|_| invalid("TLS config"))?
                .with_no_client_auth()
                .with_single_cert(certs, key)
                .map_err(|_| invalid("TLS certificate/key mismatch"))?;
            Some(tokio_rustls::TlsAcceptor::from(Arc::new(tls)))
        }
        _ => None,
    };
    let (close_sender, close_receiver) = watch::channel(false);
    let close_on_drop = ShutdownOnDrop(close_sender);
    let app = App {
        client: client.clone(),
        auth,
        config,
        events: Arc::new(Mutex::new(EventHub::new()?)),
        rate: Arc::new(Mutex::new(Rate {
            since: Instant::now(),
            count: 0,
            devices: HashMap::new(),
        })),
        ws: Arc::new(Semaphore::new(MAX_WEBSOCKETS)),
        shutdown: close_receiver,
    };
    let router = Router::new()
        .route("/v1/hello", get(hello))
        .route("/v1/pair", post(pair))
        .route("/v1/sessions", get(sessions))
        .route("/v1/projects", get(projects))
        .route("/v1/sessions/{id}", get(session))
        .route("/v1/sessions/{id}/actions", post(mutate))
        .route("/v1/sessions/{id}/screen", get(screen))
        .route("/v1/sessions/{id}/control/acquire", post(acquire))
        .route("/v1/sessions/{id}/control/release", post(release))
        .route("/v1/sessions/{id}/text", post(send_text))
        .route("/v1/devices/{id}/revoke", post(revoke))
        .route("/v1/events", get(events))
        .fallback(|| async { Failure(StatusCode::NOT_FOUND, "not_found") })
        .layer(middleware::from_fn_with_state(app.clone(), protect))
        .with_state(app.clone());
    let mut tasks = JoinSet::new();
    let event_app = app.clone();
    let mut engine_events = client.events();
    let mut connection = client.connection_state();
    tasks.spawn(async move {
        loop{tokio::select!{
            event=engine_events.recv()=>{let kind=match event{Ok(e) if e.name=="companion.changed"=>"changed",Ok(_)|Err(broadcast::error::RecvError::Lagged(_))=>"resync_required",Err(_)=>return};event_app.events.lock().unwrap().publish(kind);},
            state=connection.changed()=>{if state.is_err(){return;}let kind=if matches!(*connection.borrow_and_update(),ConnectionState::Connected(_)){"resync_required"}else{"engine_unavailable"};event_app.events.lock().unwrap().publish(kind);}
        }}
    });
    let connections = Arc::new(Semaphore::new(MAX_CONNECTIONS));
    loop {
        tokio::select! {
            _=shutdown.changed()=>{break;}
            _=tasks.join_next(),if !tasks.is_empty()=>{}
            accepted=listener.accept()=>{
                let (socket,_)=accepted?;
                let Ok(permit)=connections.clone().try_acquire_owned() else{drop(socket);continue;};
                let router=router.clone();let tls=tls.clone();
                tasks.spawn(async move{
                    let router=router.layer(axum::Extension(Arc::new(permit)));
                    let _=socket.set_nodelay(true);
                    if let Some(tls)=tls {
                        if let Ok(Ok(stream))=tokio::time::timeout(Duration::from_secs(5),tls.accept(socket)).await{connection_task(stream,router).await;}
                    }else{connection_task(socket,router).await;}
                });
            }
        }
    }
    drop(close_on_drop);
    tasks.abort_all();
    while tasks.join_next().await.is_some() {}
    let _ = tokio::time::timeout(
        Duration::from_secs(3),
        app.ws.clone().acquire_many_owned(MAX_WEBSOCKETS as u32),
    )
    .await;
    Ok(())
}
async fn connection_task<S>(stream: S, router: Router)
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let service = hyper_util::service::TowerToHyperService::new(router);
    let mut builder = hyper::server::conn::http1::Builder::new();
    builder
        .timer(hyper_util::rt::TokioTimer::new())
        .header_read_timeout(Duration::from_secs(5))
        .max_headers(32)
        .max_buf_size(32768);
    let connection = builder
        .serve_connection(hyper_util::rt::TokioIo::new(stream), service)
        .with_upgrades();
    let _ = tokio::time::timeout(Duration::from_secs(60), connection).await;
}

pub async fn bind(config: &Config) -> io::Result<TcpListener> {
    config.validate()?;
    TcpListener::bind(config.bind).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{Body, Bytes};
    use futures::StreamExt;
    use std::os::unix::fs::PermissionsExt;
    #[tokio::test]
    async fn revoked_while_decoding_cannot_reach_engine_dispatch() {
        let temp = tempfile::tempdir_in(std::fs::canonicalize("/tmp").unwrap()).unwrap();
        std::fs::set_permissions(temp.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let auth = AuthStore {
            directory: temp.path().into(),
        };
        auth.initialize().unwrap();
        let code = auth.enroll(vec![Scope::Lifecycle], now_ms()).unwrap();
        let paired = auth
            .pair(&code, "fixture", &auth.server_id().unwrap(), now_ms())
            .unwrap();
        let (_shutdown, rx) = watch::channel(false);
        let app = App {
            client: Arc::new(DaemonClient::for_companion(
                temp.path().join("missing.sock"),
            )),
            auth: auth.clone(),
            config: Config::default(),
            events: Arc::new(Mutex::new(EventHub::new().unwrap())),
            rate: Arc::new(Mutex::new(Rate {
                since: Instant::now(),
                count: 0,
                devices: HashMap::new(),
            })),
            ws: Arc::new(Semaphore::new(8)),
            shutdown: rx,
        };
        let (started, reading) = tokio::sync::oneshot::channel();
        let (finish, remaining) = tokio::sync::oneshot::channel();
        let rest = serde_json::to_vec(&Mutation {
            engine_epoch: "x".into(),
            mutation_id: "fixture-mutation-1".into(),
            expected_revision: "x".into(),
            expected_control: None,
            action: Action::Rename {
                title: "never-applied".into(),
            },
        })
        .unwrap();
        let stream = futures::stream::once(async move {
            started.send(()).unwrap();
            Ok::<Bytes, std::io::Error>(Bytes::from_static(b"{"))
        })
        .chain(futures::stream::once(async move {
            remaining.await.unwrap();
            Ok(Bytes::copy_from_slice(&rest[1..]))
        }));
        let request = Request::builder()
            .method("POST")
            .header("Content-Type", "application/json")
            .header("Authorization", format!("Bearer {}", paired.token))
            .body(Body::from_stream(stream))
            .unwrap();
        let pending = tokio::spawn(mutate(State(app), Path("s_fixture".into()), request));
        reading.await.unwrap();
        auth.revoke(&paired.device_id).unwrap();
        finish.send(()).unwrap();
        let rejection = pending.await.unwrap().err().unwrap();
        assert_eq!(rejection.0, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn slow_subscriber_overflow_is_signaled_without_blocking_publisher() {
        let mut hub = EventHub::new().unwrap();
        let mut subscriber = hub.sender.subscribe();
        for _ in 0..1000 {
            hub.publish("changed");
        }
        assert!(matches!(
            subscriber.recv().await,
            Err(broadcast::error::RecvError::Lagged(_))
        ));
        assert_eq!(hub.ring.len(), EVENT_WINDOW);
    }
}
