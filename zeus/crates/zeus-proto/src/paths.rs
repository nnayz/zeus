//! Pure path construction and shared environment names.
//!
//! This crate deliberately does not discover a home directory or touch the
//! filesystem. Callers provide the user's home directory.

use std::path::{Path, PathBuf};

pub const APP_SUPPORT_RELATIVE_PATH: &str = "Library/Application Support/Zeus";
pub const SOCKET_FILE_NAME: &str = "daemon.sock";
pub const STATE_FILE_NAME: &str = "state.json";
pub const LOGS_DIR_NAME: &str = "logs";
pub const INJECT_DIR_NAME: &str = "inject";
pub const BIN_DIR_NAME: &str = "bin";
pub const MANIFEST_OVERRIDES_RELATIVE_PATH: &str = "manifests/overrides";
pub const DAEMON_LOG_FILE_NAME: &str = "zeusd.log";
pub const HOSTS_CONFIG_FILE_NAME: &str = "hosts.json";
/// Preferences predate the Engine's capitalized support directory. Keep this
/// exact spelling: the default macOS volume is case-insensitive, but external
/// and test homes do not have to be.
pub const PREFERENCES_SUPPORT_RELATIVE_PATH: &str = "Library/Application Support/zeus";
pub const PREFERENCES_FILE_NAME: &str = "prefs.json";

pub const ENV_SESSION_ID: &str = "ZEUS_SESSION_ID";
pub const ENV_SOCKET: &str = "ZEUS_SOCKET";
pub const ENV_CLI: &str = "ZEUS_CLI";

pub struct ZeusPaths;

impl ZeusPaths {
    pub fn app_support(home: impl AsRef<Path>) -> PathBuf {
        home.as_ref().join(APP_SUPPORT_RELATIVE_PATH)
    }

    pub fn socket(home: impl AsRef<Path>) -> PathBuf {
        Self::app_support(home).join(SOCKET_FILE_NAME)
    }

    pub fn state_file(home: impl AsRef<Path>) -> PathBuf {
        Self::app_support(home).join(STATE_FILE_NAME)
    }

    pub fn logs_dir(home: impl AsRef<Path>) -> PathBuf {
        Self::app_support(home).join(LOGS_DIR_NAME)
    }

    pub fn inject_dir(home: impl AsRef<Path>) -> PathBuf {
        Self::app_support(home).join(INJECT_DIR_NAME)
    }

    pub fn bin_dir(home: impl AsRef<Path>) -> PathBuf {
        Self::app_support(home).join(BIN_DIR_NAME)
    }

    pub fn manifest_overrides_dir(home: impl AsRef<Path>) -> PathBuf {
        Self::app_support(home).join(MANIFEST_OVERRIDES_RELATIVE_PATH)
    }

    pub fn daemon_log_file(home: impl AsRef<Path>) -> PathBuf {
        Self::app_support(home).join(DAEMON_LOG_FILE_NAME)
    }

    pub fn hosts_config_file(home: impl AsRef<Path>) -> PathBuf {
        Self::app_support(home).join(HOSTS_CONFIG_FILE_NAME)
    }

    pub fn preferences_file(home: impl AsRef<Path>) -> PathBuf {
        home.as_ref()
            .join(PREFERENCES_SUPPORT_RELATIVE_PATH)
            .join(PREFERENCES_FILE_NAME)
    }
}

pub struct ZeusEnv;

impl ZeusEnv {
    pub const SESSION_ID: &'static str = ENV_SESSION_ID;
    pub const SOCKET: &'static str = ENV_SOCKET;
    pub const CLI: &'static str = ENV_CLI;
}
