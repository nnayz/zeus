use std::fmt;
use std::io;

use diri_proto::ControlError;

#[derive(Debug)]
pub enum NodeError {
    BadRequest(String),
    NotFound(String),
    Conflict(String),
    Unauthorized,
    Io(io::Error),
    Json(serde_json::Error),
    Database(rusqlite::Error),
    Provider(String),
    Protocol(String),
}

pub type NodeResult<T> = Result<T, NodeError>;

impl fmt::Display for NodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadRequest(message) => write!(formatter, "bad request: {message}"),
            Self::NotFound(message) => write!(formatter, "not found: {message}"),
            Self::Conflict(message) => write!(formatter, "conflict: {message}"),
            Self::Unauthorized => formatter.write_str("unauthorized"),
            Self::Io(error) => write!(formatter, "I/O error: {error}"),
            Self::Json(error) => write!(formatter, "JSON error: {error}"),
            Self::Database(error) => write!(formatter, "database error: {error}"),
            Self::Provider(message) => write!(formatter, "provider error: {message}"),
            Self::Protocol(message) => write!(formatter, "protocol error: {message}"),
        }
    }
}

impl std::error::Error for NodeError {}

impl From<io::Error> for NodeError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<serde_json::Error> for NodeError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

impl From<rusqlite::Error> for NodeError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Database(error)
    }
}

impl From<NodeError> for ControlError {
    fn from(error: NodeError) -> Self {
        match error {
            NodeError::BadRequest(message) => Self::bad_request(message),
            NodeError::NotFound(message) => Self::not_found(message),
            NodeError::Conflict(message) => Self::new("conflict", message),
            NodeError::Unauthorized => Self::unauthorized(),
            NodeError::Provider(message) => Self::new("provider", message),
            NodeError::Protocol(message) => Self::new("protocol", message),
            NodeError::Io(error) => Self::internal(error.to_string()),
            NodeError::Json(error) => Self::bad_request(error.to_string()),
            NodeError::Database(error) => Self::internal(error.to_string()),
        }
    }
}
