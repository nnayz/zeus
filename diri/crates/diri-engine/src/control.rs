//! The control channel: newline-delimited JSON over a Unix socket.
//!
//! This is the daemon's front door — what the app, the CLI and the MCP shim all
//! talk to. The wire format is not ours to choose: `diri-client` already speaks
//! it to the Swift daemon, so a Rust engine has to be indistinguishable on the
//! socket or every existing client breaks.
//!
//! What is implemented here is the core of that surface — handshake, list,
//! spawn, input, resize, read, kill. The rest of the method table (worktrees,
//! history, migration, hosts) is not yet ported; unknown methods return a
//! `not_found` control error, which is what an older daemon does for a method
//! it does not know, rather than dropping the connection.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use diri_proto::control::MAX_CONTROL_LINE_BYTES;
use diri_proto::{ControlError, ControlMessage, JsonValue, Method, WIRE_VERSION};
use serde_json::{Value, json};

use crate::registry::Registry;

/// Identifies this engine in the handshake, so a client can tell which
/// implementation it reached.
pub const BUILD: &str = concat!("diri-engine-", env!("CARGO_PKG_VERSION"));

pub struct ControlServer {
    registry: Arc<Mutex<Registry>>,
    socket_path: PathBuf,
    logs_dir: PathBuf,
}

impl ControlServer {
    pub fn new(registry: Arc<Mutex<Registry>>, socket_path: impl Into<PathBuf>) -> Self {
        let socket_path = socket_path.into();
        let logs_dir = socket_path
            .parent()
            .map(|parent| parent.join("logs"))
            .unwrap_or_else(|| PathBuf::from("logs"));
        Self {
            registry,
            socket_path,
            logs_dir,
        }
    }

    /// Where session output logs are written. Defaults to `logs/` beside the
    /// socket, matching the Swift daemon's layout.
    pub fn with_logs_dir(mut self, logs_dir: impl Into<PathBuf>) -> Self {
        self.logs_dir = logs_dir.into();
        self
    }

    /// Binds the socket, owner-only.
    ///
    /// The socket carries a user's terminal contents and can spawn processes as
    /// them, so the permissions are part of the security model, not a detail.
    /// A stale socket file from a dead daemon is replaced; a *live* one is not,
    /// which is what stops two engines fighting over the same endpoint.
    pub fn bind(&self) -> std::io::Result<UnixListener> {
        if self.socket_path.exists() {
            if UnixStream::connect(&self.socket_path).is_ok() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::AddrInUse,
                    format!(
                        "something is already serving {}",
                        self.socket_path.display()
                    ),
                ));
            }
            std::fs::remove_file(&self.socket_path)?;
        }
        if let Some(parent) = self.socket_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let listener = UnixListener::bind(&self.socket_path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&self.socket_path, std::fs::Permissions::from_mode(0o600))?;
        }
        Ok(listener)
    }

    /// Serves one connection to completion.
    pub fn serve(&self, stream: UnixStream) -> std::io::Result<()> {
        let reader = BufReader::new(stream.try_clone()?);
        let mut writer = stream;

        for line in reader.split(b'\n') {
            let line = line?;
            if line.is_empty() {
                continue;
            }
            if line.len() > MAX_CONTROL_LINE_BYTES {
                // A client that sends an oversized frame is out of contract;
                // answering would mean buffering unbounded input.
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "control line exceeded the protocol maximum",
                ));
            }
            let Some(response) = self.handle_line(&line) else {
                continue;
            };
            let mut bytes = serde_json::to_vec(&response)?;
            bytes.push(b'\n');
            writer.write_all(&bytes)?;
            writer.flush()?;
        }
        Ok(())
    }

    fn handle_line(&self, line: &[u8]) -> Option<ControlMessage> {
        let message: ControlMessage = match serde_json::from_slice(line) {
            Ok(message) => message,
            Err(error) => {
                // Malformed input gets an error with id 0 rather than silence:
                // a client waiting on a reply should learn it will not come.
                return Some(ControlMessage::Response {
                    id: 0,
                    result: Err(ControlError::bad_request(format!(
                        "could not parse control message: {error}"
                    ))),
                });
            }
        };

        match message {
            ControlMessage::Request { id, method, params } => Some(ControlMessage::Response {
                id,
                result: self.dispatch(&method, params),
            }),
            // Responses and events are the daemon's to send, not receive.
            ControlMessage::Response { .. } | ControlMessage::Event { .. } => None,
        }
    }

    fn dispatch(&self, method: &str, params: Option<JsonValue>) -> Result<JsonValue, ControlError> {
        match method {
            Method::HELLO => self.hello(params),
            Method::SESSION_SPAWN => self.session_spawn(params),
            Method::SESSION_LIST => self.session_list(),
            Method::SESSION_SEND_TEXT => self.session_send_text(params),
            Method::SESSION_RESIZE => self.session_resize(params),
            Method::SESSION_READ_SCREEN => self.session_read_screen(params),
            Method::SESSION_KILL => self.session_kill(params),
            other => Err(ControlError::not_found(format!(
                "method {other:?} is not implemented by this engine yet"
            ))),
        }
    }

    fn hello(&self, params: Option<JsonValue>) -> Result<JsonValue, ControlError> {
        let proto = params
            .as_ref()
            .and_then(|value| value.get("proto"))
            .and_then(Value::as_u64)
            .unwrap_or(WIRE_VERSION as u64);
        if proto != WIRE_VERSION as u64 {
            return Err(ControlError::version_mismatch(format!(
                "client speaks protocol {proto}, this engine speaks {WIRE_VERSION}"
            )));
        }
        Ok(json!({
            "proto": WIRE_VERSION,
            "build": BUILD,
            "pid": std::process::id() as i32,
        }))
    }

    /// Starts an agent and begins watching it.
    ///
    /// The command line comes from the manifest's agent descriptor, so this
    /// works for any agent that has one without code changes. Two limits worth
    /// stating: hook and MCP injection are not ported yet, so a Claude session
    /// started here is screen-detected rather than hook-driven; and `shell` and
    /// `generic` need an explicit `argv`, since their manifests declare no
    /// binary.
    fn session_spawn(&self, params: Option<JsonValue>) -> Result<JsonValue, ControlError> {
        let params = params.ok_or_else(|| ControlError::bad_request("params are required"))?;
        let kind = string_field(&params, "kind")?;
        let cwd = string_field(&params, "cwd")?;
        let cwd_path = PathBuf::from(&cwd);
        if !cwd_path.is_dir() {
            return Err(ControlError::bad_request(format!(
                "cwd {cwd:?} is not a directory"
            )));
        }
        let argv: Vec<String> = params
            .get("argv")
            .and_then(Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();

        let mut registry = self.registry.lock().map_err(poisoned)?;
        let engine = registry.engine();
        let manifest = engine
            .manifest(&kind)
            .ok_or_else(|| ControlError::not_found(format!("no manifest for agent {kind:?}")))?;
        let descriptor = manifest.agent.clone().unwrap_or_default();
        let authority = descriptor.authority();

        let inherited: Vec<(String, String)> = std::env::vars().collect();
        let pty = match descriptor.spawn_spec(&cwd_path, inherited.clone(), &argv) {
            Some(spec) => spec,
            // No binary in the manifest: the caller has to say what to run.
            None if !argv.is_empty() => {
                let mut spec = crate::pty::PtySpec::new(argv.clone(), &cwd_path);
                spec.env = inherited;
                spec.env.retain(|(key, _)| key != "NO_COLOR");
                spec
            }
            None => {
                return Err(ControlError::bad_request(format!(
                    "agent {kind:?} declares no binary, so argv is required"
                )));
            }
        };

        let id = next_session_id();
        let record = new_record(&id, &kind, &cwd);
        let spec = crate::session::SessionSpec {
            id: id.clone(),
            pty,
            manifest_id: kind,
            authority,
            logs_dir: self.logs_dir.clone(),
        };
        registry
            .spawn(spec, record)
            .map_err(|error| ControlError::internal(error.to_string()))?;
        let _ = registry.persist();

        let record = registry
            .records()
            .into_iter()
            .find(|record| record.id.0 == id)
            .ok_or_else(|| ControlError::internal("the new session vanished"))?;
        serde_json::to_value(json!({ "session": record }))
            .map_err(|error| ControlError::internal(error.to_string()))
    }

    fn session_list(&self) -> Result<JsonValue, ControlError> {
        let registry = self.registry.lock().map_err(poisoned)?;
        serde_json::to_value(json!({ "sessions": registry.records() }))
            .map_err(|error| ControlError::internal(error.to_string()))
    }

    fn session_send_text(&self, params: Option<JsonValue>) -> Result<JsonValue, ControlError> {
        let params = params.ok_or_else(|| ControlError::bad_request("params are required"))?;
        let id = string_field(&params, "id")?;
        let text = string_field(&params, "text")?;

        let registry = self.registry.lock().map_err(poisoned)?;
        let session = registry
            .get(&id)
            .ok_or_else(|| ControlError::not_found(format!("no session {id}")))?;
        session
            .write_input(text.as_bytes())
            .map_err(|error| ControlError::internal(error.to_string()))?;
        Ok(json!({}))
    }

    fn session_resize(&self, params: Option<JsonValue>) -> Result<JsonValue, ControlError> {
        let params = params.ok_or_else(|| ControlError::bad_request("params are required"))?;
        let id = string_field(&params, "id")?;
        let cols = u16_field(&params, "cols")?;
        let rows = u16_field(&params, "rows")?;

        let registry = self.registry.lock().map_err(poisoned)?;
        let session = registry
            .get(&id)
            .ok_or_else(|| ControlError::not_found(format!("no session {id}")))?;
        session
            .resize(cols, rows)
            .map_err(|error| ControlError::internal(error.to_string()))?;
        Ok(json!({}))
    }

    fn session_read_screen(&self, params: Option<JsonValue>) -> Result<JsonValue, ControlError> {
        let params = params.ok_or_else(|| ControlError::bad_request("params are required"))?;
        let id = string_field(&params, "id")?;

        let registry = self.registry.lock().map_err(poisoned)?;
        let session = registry
            .get(&id)
            .ok_or_else(|| ControlError::not_found(format!("no session {id}")))?;
        Ok(json!({ "lines": session.screen_lines() }))
    }

    fn session_kill(&self, params: Option<JsonValue>) -> Result<JsonValue, ControlError> {
        let params = params.ok_or_else(|| ControlError::bad_request("params are required"))?;
        let id = string_field(&params, "id")?;

        let mut registry = self.registry.lock().map_err(poisoned)?;
        let exit = registry
            .terminate(&id, std::time::Duration::from_secs(3))
            .map_err(|error| ControlError::internal(error.to_string()))?;
        if exit.is_none() {
            return Err(ControlError::not_found(format!("no session {id}")));
        }
        let _ = registry.persist();
        Ok(json!({}))
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }
}

impl Drop for ControlServer {
    fn drop(&mut self) {
        // Leaving the socket file behind would make the next start think a
        // daemon is already running.
        let _ = std::fs::remove_file(&self.socket_path);
    }
}

/// A session id in the daemon's format: `s_` plus twelve hex digits.
fn next_session_id() -> String {
    let mut bytes = [0u8; 6];
    getrandom::fill(&mut bytes).expect("the OS random source");
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    format!("s_{hex}")
}

fn new_record(id: &str, kind: &str, cwd: &str) -> diri_proto::SessionRecord {
    use diri_proto::{AgentKind, DateMillis, ProjectId, Resumability, SessionId, TitleSource};
    let now: DateMillis = std::time::SystemTime::now().into();
    diri_proto::SessionRecord {
        id: SessionId(id.to_string()),
        kind: AgentKind::new(kind),
        cwd: cwd.to_string(),
        project_id: ProjectId(cwd.to_string()),
        worktree_path: None,
        git_branch: None,
        title: kind.to_string(),
        title_source: TitleSource::Placeholder,
        agent_session_id: None,
        transcript_path: None,
        status: diri_proto::SessionStatus::Starting,
        needs_input: None,
        resumability: Resumability::Live,
        parent: None,
        created_at: now,
        updated_at: now,
        last_turn_completed_at: None,
        last_seen_at: None,
        pinned: false,
        archived_at: None,
        remote_active: false,
        host: None,
        hibernation: None,
        memory_bytes: None,
        artifacts: None,
        pull_requests: None,
        listening_ports: None,
        foreground_agent: None,
    }
}

fn poisoned<T>(_: T) -> ControlError {
    ControlError::internal("engine state is poisoned")
}

fn string_field(params: &JsonValue, name: &str) -> Result<String, ControlError> {
    params
        .get(name)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| ControlError::bad_request(format!("{name} must be a string")))
}

fn u16_field(params: &JsonValue, name: &str) -> Result<u16, ControlError> {
    params
        .get(name)
        .and_then(Value::as_u64)
        .and_then(|value| u16::try_from(value).ok())
        .ok_or_else(|| ControlError::bad_request(format!("{name} must fit in a u16")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::ManifestEngine;

    fn engine() -> Arc<ManifestEngine> {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../Sources/DirijorCore/Resources/manifests")
            .canonicalize()
            .expect("manifests");
        let (engine, _) = ManifestEngine::load_dir(&dir).expect("load");
        Arc::new(engine)
    }

    fn server(temp: &Path) -> ControlServer {
        let registry = Registry::new(engine(), temp.join("state.json"));
        ControlServer::new(Arc::new(Mutex::new(registry)), temp.join("daemon.sock"))
    }

    /// Round-trips one request through the dispatcher the way a client would.
    fn call(server: &ControlServer, method: &str, params: Option<JsonValue>) -> ControlMessage {
        let request = ControlMessage::Request {
            id: 1,
            method: method.into(),
            params,
        };
        let line = serde_json::to_vec(&request).expect("encode");
        server
            .handle_line(&line)
            .expect("a request gets a response")
    }

    fn ok_of(message: ControlMessage) -> JsonValue {
        match message {
            ControlMessage::Response { result: Ok(ok), .. } => ok,
            other => panic!("expected success, got {other:?}"),
        }
    }

    fn err_of(message: ControlMessage) -> ControlError {
        match message {
            ControlMessage::Response {
                result: Err(error), ..
            } => error,
            other => panic!("expected an error, got {other:?}"),
        }
    }

    #[test]
    fn hello_reports_the_protocol_and_the_engine_build() {
        let temp = tempfile::tempdir().expect("temp");
        let server = server(temp.path());
        let result = ok_of(call(
            &server,
            "hello",
            Some(json!({ "proto": WIRE_VERSION, "build": "test-client" })),
        ));

        assert_eq!(result["proto"], WIRE_VERSION);
        assert!(
            result["build"]
                .as_str()
                .is_some_and(|b| b.contains("diri-engine")),
            "the handshake should say which engine answered: {result}"
        );
        assert!(result["pid"].as_i64().is_some_and(|pid| pid > 0));
    }

    #[test]
    fn a_client_on_another_protocol_is_told_so() {
        let temp = tempfile::tempdir().expect("temp");
        let server = server(temp.path());
        let error = err_of(call(
            &server,
            "hello",
            Some(json!({ "proto": 99, "build": "future-client" })),
        ));
        assert_eq!(error.code, "version_mismatch");
    }

    #[test]
    fn listing_sessions_returns_the_records() {
        let temp = tempfile::tempdir().expect("temp");
        let server = server(temp.path());
        let result = ok_of(call(&server, "session.list", None));
        assert!(result["sessions"].is_array());
    }

    #[test]
    fn an_unimplemented_method_is_not_found_rather_than_a_dropped_connection() {
        // A client that asks for something this engine has not ported yet must
        // get a clean error, the same as an older daemon would give.
        let temp = tempfile::tempdir().expect("temp");
        let server = server(temp.path());
        let error = err_of(call(&server, "worktree.create", Some(json!({}))));
        assert_eq!(error.code, "not_found");
    }

    #[test]
    fn addressing_a_session_that_does_not_exist_is_an_error() {
        let temp = tempfile::tempdir().expect("temp");
        let server = server(temp.path());
        let error = err_of(call(
            &server,
            "session.send_text",
            Some(json!({ "id": "s_missing", "text": "hi" })),
        ));
        assert_eq!(error.code, "not_found");
    }

    #[test]
    fn missing_parameters_are_rejected_before_anything_happens() {
        let temp = tempfile::tempdir().expect("temp");
        let server = server(temp.path());
        assert_eq!(
            err_of(call(&server, "session.send_text", None)).code,
            "bad_request"
        );
        assert_eq!(
            err_of(call(&server, "session.resize", Some(json!({ "id": "s" })))).code,
            "bad_request"
        );
    }

    #[test]
    fn malformed_json_gets_an_error_rather_than_silence() {
        // A client waiting on a reply should learn that none is coming.
        let temp = tempfile::tempdir().expect("temp");
        let server = server(temp.path());
        let response = server.handle_line(b"{ not json").expect("a response");
        assert_eq!(err_of(response).code, "bad_request");
    }

    #[test]
    fn responses_and_events_from_a_client_are_ignored() {
        let temp = tempfile::tempdir().expect("temp");
        let server = server(temp.path());
        let event = serde_json::to_vec(&ControlMessage::Event {
            name: "session.updated".into(),
            seq: 1,
            params: json!({}),
        })
        .expect("encode");
        assert!(
            server.handle_line(&event).is_none(),
            "the daemon sends events; it does not answer them"
        );
    }

    #[test]
    fn the_socket_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().expect("temp");
        let server = server(temp.path());
        let _listener = server.bind().expect("bind");

        let mode = std::fs::metadata(server.socket_path())
            .expect("stat")
            .permissions()
            .mode();
        assert_eq!(
            mode & 0o777,
            0o600,
            "the control socket can spawn processes as the user"
        );
    }

    #[test]
    fn binding_over_a_live_socket_is_refused() {
        let temp = tempfile::tempdir().expect("temp");
        let server = server(temp.path());
        let _listener = server.bind().expect("first bind");

        let second = ControlServer::new(
            Arc::new(Mutex::new(Registry::new(
                engine(),
                temp.path().join("state.json"),
            ))),
            server.socket_path(),
        );
        let error = second
            .bind()
            .expect_err("two engines must not share a socket");
        assert_eq!(error.kind(), std::io::ErrorKind::AddrInUse);
    }

    #[test]
    fn a_stale_socket_file_is_replaced() {
        // The daemon died without cleaning up; the next start must not be
        // blocked by the leftover file.
        let temp = tempfile::tempdir().expect("temp");
        let path = temp.path().join("daemon.sock");
        std::fs::write(&path, b"").expect("leave a stale file");

        let server = ControlServer::new(
            Arc::new(Mutex::new(Registry::new(
                engine(),
                temp.path().join("state.json"),
            ))),
            &path,
        );
        let _listener = server.bind().expect("a stale socket should be replaced");
    }
}
