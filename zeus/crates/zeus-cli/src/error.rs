use std::fmt;
use std::io;

use zeus_proto::ControlError;

pub const EXIT_FAILURE: i32 = 1;
pub const EXIT_TIMEOUT: i32 = 2;
pub const EXIT_NOT_FOUND: i32 = 3;
pub const EXIT_UNREACHABLE: i32 = 4;

#[derive(Debug)]
pub enum CliError {
    Usage(String),
    Failure(String),
    Timeout,
    NotFound(String),
    Unreachable(String),
}

impl CliError {
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::Usage(_) | Self::Failure(_) => EXIT_FAILURE,
            Self::Timeout => EXIT_TIMEOUT,
            Self::NotFound(_) => EXIT_NOT_FOUND,
            Self::Unreachable(_) => EXIT_UNREACHABLE,
        }
    }

    pub fn from_control(error: ControlError) -> Self {
        if error.code == "not_found" {
            Self::NotFound(format!("{}: {}", error.code, error.message))
        } else {
            Self::Failure(format!("{}: {}", error.code, error.message))
        }
    }

    pub fn from_io(error: io::Error) -> Self {
        if error.kind() == io::ErrorKind::TimedOut {
            Self::Timeout
        } else {
            Self::Failure(error.to_string())
        }
    }
}

impl fmt::Display for CliError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Usage(message) | Self::Failure(message) | Self::NotFound(message) => {
                write!(formatter, "{message}")
            }
            Self::Timeout => write!(formatter, "timed out"),
            Self::Unreachable(message) => write!(formatter, "daemon unreachable ({message})"),
        }
    }
}

impl std::error::Error for CliError {}

impl From<io::Error> for CliError {
    fn from(error: io::Error) -> Self {
        Self::from_io(error)
    }
}

impl From<serde_json::Error> for CliError {
    fn from(error: serde_json::Error) -> Self {
        Self::Failure(error.to_string())
    }
}
