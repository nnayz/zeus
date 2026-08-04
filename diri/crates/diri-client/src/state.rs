use diri_proto::control::JsonValue;
use diri_proto::methods::HelloResult;

/// Observable state of the daemon control connection.
#[derive(Clone, Debug, PartialEq)]
pub enum ConnectionState {
    Connecting,
    Connected(HelloResult),
    Disconnected(String),
}

/// A sequence-stamped event delivered by the daemon.
#[derive(Clone, Debug, PartialEq)]
pub struct EventEnvelope {
    pub name: String,
    pub seq: u64,
    pub params: JsonValue,
}
