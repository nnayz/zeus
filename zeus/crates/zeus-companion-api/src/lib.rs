//! Companion v1: curated external DTOs. No dependency on the Engine wire model.
use serde::{Deserialize, Serialize};

pub const API_MAJOR: u32 = 1;
pub const API_MINOR: u32 = 0;
pub const MAX_BODY: usize = 16 * 1024;
pub const MAX_RESPONSE: usize = 256 * 1024;
pub const MAX_TEXT: usize = 8 * 1024;
pub const MAX_PAGE: usize = 64;
pub const MAX_STRING: usize = 512;
pub const EVENT_WINDOW: usize = 128;
pub const CAPABILITIES: &[&str] = &[
    "projects",
    "sessions",
    "screen",
    "events",
    "rename",
    "lifecycle",
    "control_lease",
    "send_text",
];

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    Read,
    Interact,
    Spawn,
    Lifecycle,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Hello {
    pub server_id: String,
    pub api_major: u32,
    pub api_minor: u32,
    pub capabilities: Vec<String>,
    pub engine_epoch: String,
    pub max_body_bytes: usize,
    pub max_response_bytes: usize,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PairRequest {
    pub api_major: u32,
    pub expected_server_id: String,
    pub code: String,
    pub device_name: String,
}

#[derive(Clone, Deserialize, Serialize)]
pub struct PairResponse {
    pub server_id: String,
    pub device_id: String,
    pub token: String,
    pub scopes: Vec<Scope>,
    pub expires_at_ms: u64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Device {
    pub id: String,
    pub name: String,
    pub scopes: Vec<Scope>,
    pub expires_at_ms: u64,
    pub revoked: bool,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PageRequest {
    #[serde(default)]
    pub offset: usize,
    pub limit: Option<usize>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next_offset: Option<usize>,
    pub engine_epoch: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub root: String,
    pub host: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Session {
    pub id: String,
    pub project_id: String,
    pub kind: String,
    pub title: String,
    pub cwd: String,
    pub host: Option<String>,
    pub status: String,
    pub created_at_ms: f64,
    pub updated_at_ms: f64,
    pub archived: bool,
    pub hibernated: bool,
    /// Engine-issued precondition; refresh before a new mutation.
    pub revision: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct SessionDetail {
    pub session: Session,
    pub engine_epoch: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Mutation {
    pub engine_epoch: String,
    /// Unique 16..64 byte ASCII identifier. Never reuse for another operation.
    pub mutation_id: String,
    pub expected_revision: String,
    pub expected_control: Option<ControlEpoch>,
    pub action: Action,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    Rename { title: String },
    Archive { confirmed: bool },
    Wake { confirmed: bool },
    Hibernate { confirmed: bool },
    Terminate { confirmed: bool },
}

impl std::fmt::Debug for Action {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Rename { .. } => "Rename([redacted])",
            Self::Archive { .. } => "Archive",
            Self::Wake { .. } => "Wake",
            Self::Hibernate { .. } => "Hibernate",
            Self::Terminate { .. } => "Terminate",
        })
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct MutationResult {
    pub mutation_id: String,
    pub applied: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ControlEpoch {
    pub incarnation: String,
    pub generation: u64,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Controller {
    pub id: String,
    pub label: String,
    pub role: String,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ControlState {
    pub epoch: ControlEpoch,
    pub owner: Option<Controller>,
    pub command_seq: u64,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AcquireControl {
    pub expected: ControlEpoch,
    pub takeover: bool,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseControl {
    pub expected: ControlEpoch,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SendText {
    pub expected: ControlEpoch,
    pub command_seq: u64,
    pub text: String,
    pub submit: bool,
}
#[derive(Clone, Deserialize, PartialEq, Serialize)]
pub struct Screen {
    pub session_id: String,
    pub incarnation: String,
    pub screen_sequence: u64,
    pub text: String,
    pub cols: u16,
    pub rows: u16,
    pub cursor_row: u16,
    pub cursor_col: u16,
    pub control: ControlState,
    pub exited: bool,
    pub truncated: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Cursor {
    pub stream_id: String,
    pub sequence: u64,
}

/// First WebSocket client frame. Authentication material is never in a URL.
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Subscribe {
    pub api_major: u32,
    pub token: String,
    pub cursor: Option<Cursor>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Event {
    pub cursor: Cursor,
    /// `changed`, `resync_required`, or `engine_unavailable`.
    /// Notifications invalidate projections; consumers fetch fresh bounded state.
    pub kind: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ApiError {
    pub code: String,
}

pub fn bounded_string(value: &str, limit: usize) -> String {
    let mut end = value.len().min(limit);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_owned()
}

pub fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}
