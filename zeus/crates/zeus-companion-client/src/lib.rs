//! Typed, bounded Companion reference client.
use futures::{SinkExt, StreamExt};
use reqwest::{Method, StatusCode};
use serde::{Serialize, de::DeserializeOwned};
use std::time::Duration;
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream,
    tungstenite::{Message, protocol::WebSocketConfig},
};
use zeroize::Zeroizing;
use zeus_companion_api::*;

const DEADLINE: Duration = Duration::from_secs(10);
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Error {
    pub code: String,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.code)
    }
}
impl std::error::Error for Error {}
fn error(code: &str) -> Error {
    Error { code: code.into() }
}

pub struct Client {
    http: reqwest::Client,
    origin: url::Url,
    token: Zeroizing<String>,
    server_id: String,
}
impl Client {
    pub fn new(origin: &str, token: String, expected_server_id: String) -> Result<Self, Error> {
        let origin = url::Url::parse(origin).map_err(|_| error("invalid_origin"))?;
        let loopback = origin.host_str().is_some_and(|s| {
            s.trim_matches(['[', ']'])
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback())
        });
        if !(origin.scheme() == "https" || (origin.scheme() == "http" && loopback))
            || origin.username() != ""
            || origin.password().is_some()
            || origin.query().is_some()
            || origin.fragment().is_some()
            || origin.path() != "/"
        {
            return Err(error("invalid_origin"));
        }
        if token.len() != 64 || expected_server_id.len() != 64 {
            return Err(error("invalid_credentials"));
        }
        let http = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(5))
            .timeout(DEADLINE)
            .pool_max_idle_per_host(2)
            .build()
            .map_err(|_| error("transport_unavailable"))?;
        Ok(Self {
            http,
            origin,
            token: Zeroizing::new(token),
            server_id: expected_server_id,
        })
    }
    pub async fn pair(origin: &str, request: &PairRequest) -> Result<PairResponse, Error> {
        let client = Self::new(origin, "0".repeat(64), request.expected_server_id.clone())?;
        let response: PairResponse = client
            .request(Method::POST, "/v1/pair", Some(request), false)
            .await?;
        if response.server_id != client.server_id {
            return Err(error("wrong_server"));
        }
        Ok(response)
    }
    pub async fn hello(&self, required: &[&str]) -> Result<Hello, Error> {
        let hello: Hello = self.get("/v1/hello").await?;
        if hello.api_major != API_MAJOR {
            return Err(error("version_mismatch"));
        }
        if hello.server_id != self.server_id {
            return Err(error("wrong_server"));
        }
        if required
            .iter()
            .any(|required| !hello.capabilities.iter().any(|c| c == required))
        {
            return Err(error("capability_unavailable"));
        }
        Ok(hello)
    }
    pub async fn sessions(&self, page: &PageRequest) -> Result<Page<Session>, Error> {
        self.get(&page_path("sessions", page)?).await
    }
    pub async fn projects(&self, page: &PageRequest) -> Result<Page<Project>, Error> {
        self.get(&page_path("projects", page)?).await
    }
    pub async fn session(&self, id: &str) -> Result<SessionDetail, Error> {
        self.get(&session_path(id, "")?).await
    }
    pub async fn screen(&self, id: &str) -> Result<Screen, Error> {
        self.get(&session_path(id, "/screen")?).await
    }
    /// Mutations are sent exactly once. Cancellation drops the request future;
    /// uncertain outcomes require a fresh projection, never blind resubmission.
    pub async fn mutate(&self, id: &str, mutation: &Mutation) -> Result<MutationResult, Error> {
        self.request(
            Method::POST,
            &session_path(id, "/actions")?,
            Some(mutation),
            true,
        )
        .await
    }
    pub async fn acquire(&self, id: &str, request: &AcquireControl) -> Result<ControlState, Error> {
        self.request(
            Method::POST,
            &session_path(id, "/control/acquire")?,
            Some(request),
            true,
        )
        .await
    }
    pub async fn release(&self, id: &str, request: &ReleaseControl) -> Result<ControlState, Error> {
        self.request(
            Method::POST,
            &session_path(id, "/control/release")?,
            Some(request),
            true,
        )
        .await
    }
    pub async fn send_text(&self, id: &str, request: &SendText) -> Result<ControlState, Error> {
        if request.text.len() > MAX_TEXT {
            return Err(error("body_limit"));
        }
        self.request(
            Method::POST,
            &session_path(id, "/text")?,
            Some(request),
            true,
        )
        .await
    }
    pub async fn revoke_self(&self, id: &str) -> Result<(), Error> {
        if !valid_id(id) {
            return Err(error("invalid_id"));
        }
        let _: serde_json::Value = self
            .request(
                Method::POST,
                &format!("/v1/devices/{id}/revoke"),
                Some(&serde_json::json!({})),
                true,
            )
            .await?;
        Ok(())
    }
    async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T, Error> {
        self.request::<(), T>(Method::GET, path, None, true).await
    }
    async fn request<P: Serialize + ?Sized, T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: Option<&P>,
        authenticated: bool,
    ) -> Result<T, Error> {
        let url = self.origin.join(path).map_err(|_| error("invalid_path"))?;
        let mut request = self.http.request(method, url);
        if authenticated {
            request = request.bearer_auth(self.token.as_str());
        }
        if let Some(body) = body {
            let bytes = serde_json::to_vec(body).map_err(|_| error("invalid_request"))?;
            if bytes.len() > MAX_BODY {
                return Err(error("body_limit"));
            }
            request = request
                .header("Content-Type", "application/json")
                .body(bytes);
        }
        let response = request.send().await.map_err(|_| error("outcome_unknown"))?;
        let status = response.status();
        if response
            .content_length()
            .is_some_and(|n| n > MAX_RESPONSE as u64)
        {
            return Err(error("response_limit"));
        }
        let mut bytes = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| error("outcome_unknown"))?;
            if bytes.len().saturating_add(chunk.len()) > MAX_RESPONSE {
                return Err(error("response_limit"));
            }
            bytes.extend_from_slice(&chunk);
        }
        if !status.is_success() {
            return Err(response_error(status, &bytes));
        }
        serde_json::from_slice(&bytes).map_err(|_| error("invalid_response"))
    }
    /// Reconnect by passing the last cursor; a `resync_required` event means
    /// fetch fresh pages/screens. This never replays a mutation.
    pub async fn subscribe(&self, cursor: Option<Cursor>) -> Result<Events, Error> {
        let mut url = self
            .origin
            .join("/v1/events")
            .map_err(|_| error("invalid_path"))?;
        url.set_scheme(if self.origin.scheme() == "https" {
            "wss"
        } else {
            "ws"
        })
        .map_err(|_| error("invalid_origin"))?;
        let config = WebSocketConfig::default()
            .max_message_size(Some(MAX_RESPONSE))
            .max_frame_size(Some(MAX_RESPONSE))
            .max_write_buffer_size(MAX_BODY * 2)
            .write_buffer_size(0);
        let (mut socket, _) = tokio::time::timeout(
            DEADLINE,
            tokio_tungstenite::connect_async_with_config(url.as_str(), Some(config), true),
        )
        .await
        .map_err(|_| error("timeout"))?
        .map_err(|_| error("connection_failed"))?;
        let frame = Subscribe {
            api_major: API_MAJOR,
            token: self.token.to_string(),
            cursor,
        };
        let json =
            Zeroizing::new(serde_json::to_string(&frame).map_err(|_| error("invalid_request"))?);
        tokio::time::timeout(
            DEADLINE,
            socket.send(Message::Text(json.to_string().into())),
        )
        .await
        .map_err(|_| error("timeout"))?
        .map_err(|_| error("connection_failed"))?;
        Ok(Events {
            socket,
            cursor: None,
        })
    }
}
fn response_error(status: StatusCode, bytes: &[u8]) -> Error {
    if let Ok(api) = serde_json::from_slice::<ApiError>(bytes)
        && api.code.len() <= 64
        && api
            .code
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b == b'_')
    {
        return error(&api.code);
    }
    error(if status == StatusCode::UNAUTHORIZED {
        "unauthorized"
    } else {
        "request_failed"
    })
}
fn session_path(id: &str, suffix: &str) -> Result<String, Error> {
    if !valid_id(id) {
        return Err(error("invalid_id"));
    }
    Ok(format!("/v1/sessions/{id}{suffix}"))
}
fn page_path(kind: &str, page: &PageRequest) -> Result<String, Error> {
    let limit = page.limit.unwrap_or(MAX_PAGE);
    if page.offset > 8192 || limit == 0 || limit > MAX_PAGE {
        return Err(error("invalid_page"));
    }
    Ok(format!("/v1/{kind}?offset={}&limit={limit}", page.offset))
}
pub struct Events {
    socket: WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>,
    pub cursor: Option<Cursor>,
}
impl Events {
    pub async fn next(&mut self) -> Result<Event, Error> {
        loop {
            match self.socket.next().await {
                Some(Ok(Message::Text(text))) => {
                    let event: Event =
                        serde_json::from_str(&text).map_err(|_| error("invalid_event"))?;
                    if event.cursor.stream_id.len() > 64
                        || !["changed", "resync_required", "engine_unavailable"]
                            .contains(&event.kind.as_str())
                    {
                        return Err(error("invalid_event"));
                    }
                    self.cursor = Some(event.cursor.clone());
                    return Ok(event);
                }
                Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
                _ => return Err(error("connection_closed")),
            }
        }
    }
}
