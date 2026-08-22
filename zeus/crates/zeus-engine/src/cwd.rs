//! Working-directory validation shared by the Engine and local Holder.
//!
//! `Path::is_dir()` deliberately erases every I/O error into `false`. That is
//! wrong at a process boundary: a missing checkout and a macOS TCC denial need
//! different remediation, and the latter was reaching the login shell as the
//! misleading "Current directory does not exist".

use std::fmt;
use std::path::Path;

use serde::{Deserialize, Serialize};

pub const NOT_FOUND: &str = "cwd_not_found";
pub const NOT_DIRECTORY: &str = "cwd_not_directory";
pub const PERMISSION_DENIED: &str = "cwd_permission_denied";
pub const UNREADABLE: &str = "cwd_unreadable";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CwdAccessError {
    pub code: String,
    pub message: String,
}

impl CwdAccessError {
    fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_owned(),
            message: message.into(),
        }
    }

    fn from_io(path: &Path, error: std::io::Error) -> Self {
        match error.kind() {
            std::io::ErrorKind::NotFound => Self::new(
                NOT_FOUND,
                format!("working directory {:?} does not exist", path.display()),
            ),
            std::io::ErrorKind::NotADirectory => Self::new(
                NOT_DIRECTORY,
                format!(
                    "working directory {:?} contains a component that is not a directory",
                    path.display()
                ),
            ),
            std::io::ErrorKind::PermissionDenied => Self::permission_denied(path, &error),
            _ if matches!(error.raw_os_error(), Some(libc::EACCES | libc::EPERM)) => {
                Self::permission_denied(path, &error)
            }
            _ => Self::new(
                UNREADABLE,
                format!(
                    "Zeus could not read working directory {:?}: {error}",
                    path.display()
                ),
            ),
        }
    }

    fn permission_denied(path: &Path, error: &std::io::Error) -> Self {
        #[cfg(target_os = "macos")]
        let remedy = " Allow Zeus access in System Settings → Privacy & Security → Files & Folders, then restart Zeus.";
        #[cfg(not(target_os = "macos"))]
        let remedy = " Check the directory permissions for the user running Zeus.";
        Self::new(
            PERMISSION_DENIED,
            format!(
                "Zeus cannot read working directory {:?}: {error}.{remedy}",
                path.display()
            ),
        )
    }

    #[must_use]
    pub fn io_kind(&self) -> std::io::ErrorKind {
        match self.code.as_str() {
            NOT_FOUND => std::io::ErrorKind::NotFound,
            NOT_DIRECTORY => std::io::ErrorKind::NotADirectory,
            PERMISSION_DENIED => std::io::ErrorKind::PermissionDenied,
            _ => std::io::ErrorKind::Other,
        }
    }
}

impl fmt::Display for CwdAccessError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for CwdAccessError {}

impl From<CwdAccessError> for std::io::Error {
    fn from(error: CwdAccessError) -> Self {
        std::io::Error::new(error.io_kind(), error)
    }
}

/// Confirms that `path` exists, is a directory, and can be enumerated by the
/// current process. Opening `read_dir` is intentional: metadata commonly
/// succeeds across a macOS TCC boundary while enumeration returns `EPERM`.
pub fn validate_directory(path: &Path) -> Result<(), CwdAccessError> {
    let metadata = std::fs::metadata(path).map_err(|error| CwdAccessError::from_io(path, error))?;
    if !metadata.is_dir() {
        return Err(CwdAccessError::new(
            NOT_DIRECTORY,
            format!("working directory {:?} is not a directory", path.display()),
        ));
    }
    std::fs::read_dir(path)
        .map(|_| ())
        .map_err(|error| CwdAccessError::from_io(path, error))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_and_non_directory_paths_are_distinct() {
        let temporary = tempfile::tempdir().expect("tempdir");
        let missing = temporary.path().join("missing");
        let error = validate_directory(&missing).expect_err("missing");
        assert_eq!(error.code, NOT_FOUND);

        let file = temporary.path().join("file");
        std::fs::write(&file, b"not a directory").expect("fixture file");
        let error = validate_directory(&file).expect_err("file");
        assert_eq!(error.code, NOT_DIRECTORY);
    }

    #[test]
    fn eperm_is_an_actionable_permission_error() {
        let path = Path::new("/protected/project");
        let error = CwdAccessError::from_io(path, std::io::Error::from_raw_os_error(libc::EPERM));
        assert_eq!(error.code, PERMISSION_DENIED);
        assert!(error.message.contains("/protected/project"));
        assert_eq!(error.io_kind(), std::io::ErrorKind::PermissionDenied);
    }
}
