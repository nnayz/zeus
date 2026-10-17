//! Pure path construction and shared environment names.
//!
//! This crate deliberately does not discover a home directory or touch the
//! filesystem. Callers provide the user's home directory. The state-root
//! layout is resolved here — including the `ZEUS_APP_SUPPORT` override and the
//! Linux XDG state directory — so the Engine, CLI, app, and client always
//! agree on one root.

use std::path::{Path, PathBuf};

/// macOS application-support layout. The desktop app is macOS-only; the
/// Linux headless Engine follows the XDG state directory instead.
pub const APP_SUPPORT_RELATIVE_PATH: &str = "Library/Application Support/Zeus";
/// Linux state root under `$XDG_STATE_HOME` (defaulting to `~/.local/state`).
/// The leaf matches the remote Helper's `~/.local/state/zeus` session layout.
pub const XDG_STATE_RELATIVE_PATH: &str = ".local/state";
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
/// Overrides the Engine state root. Set by headless deployments, secondary
/// engines, and tests so the Engine, CLI, and client share one location.
pub const ENV_APP_SUPPORT: &str = "ZEUS_APP_SUPPORT";
/// Standard XDG state home. Only consulted on Linux.
pub const ENV_XDG_STATE_HOME: &str = "XDG_STATE_HOME";

pub struct ZeusPaths;

impl ZeusPaths {
    /// Resolve the Engine state root from explicit values, without reading the
    /// environment. `app_support_override` is the `ZEUS_APP_SUPPORT` value;
    /// `xdg_state_home` is `XDG_STATE_HOME` and is only consulted on Linux.
    ///
    /// A relative `XDG_STATE_HOME` is ignored, as the XDG base directory
    /// specification requires.
    pub fn app_support_from(
        home: impl AsRef<Path>,
        app_support_override: Option<&Path>,
        xdg_state_home: Option<&Path>,
    ) -> PathBuf {
        if let Some(root) = app_support_override.filter(|root| !root.as_os_str().is_empty()) {
            return root.to_path_buf();
        }
        #[cfg(target_os = "linux")]
        let root = {
            let default_state = home.as_ref().join(XDG_STATE_RELATIVE_PATH);
            let state = xdg_state_home
                .filter(|root| !root.as_os_str().is_empty() && root.is_absolute())
                .unwrap_or(default_state.as_path());
            state.join("zeus")
        };
        #[cfg(not(target_os = "linux"))]
        let root = {
            let _ = xdg_state_home;
            home.as_ref().join(APP_SUPPORT_RELATIVE_PATH)
        };
        root
    }

    /// The Engine state root for `home`: `ZEUS_APP_SUPPORT` when set and
    /// non-empty, otherwise the platform default — the macOS application
    /// support directory, or `$XDG_STATE_HOME/zeus` (default
    /// `~/.local/state/zeus`) on Linux.
    pub fn app_support(home: impl AsRef<Path>) -> PathBuf {
        let app_support = std::env::var_os(ENV_APP_SUPPORT).filter(|value| !value.is_empty());
        let xdg_state_home = std::env::var_os(ENV_XDG_STATE_HOME).filter(|value| !value.is_empty());
        Self::app_support_from(
            home,
            app_support.as_deref().map(Path::new),
            xdg_state_home.as_deref().map(Path::new),
        )
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
    pub const APP_SUPPORT: &'static str = ENV_APP_SUPPORT;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_app_support_override_wins_on_every_platform() {
        assert_eq!(
            ZeusPaths::app_support_from(
                "/home/user",
                Some(Path::new("/custom/state")),
                Some(Path::new("/xdg-state")),
            ),
            PathBuf::from("/custom/state")
        );
        assert_eq!(
            ZeusPaths::app_support_from("/home/user", Some(Path::new("")), None),
            platform_default("/home/user")
        );
    }

    #[cfg(target_os = "linux")]
    fn platform_default(home: &str) -> PathBuf {
        PathBuf::from(home).join(".local/state/zeus")
    }

    #[cfg(not(target_os = "linux"))]
    fn platform_default(home: &str) -> PathBuf {
        PathBuf::from(home).join("Library/Application Support/Zeus")
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_uses_the_xdg_state_directory() {
        assert_eq!(
            ZeusPaths::app_support_from("/home/user", None, Some(Path::new("/xdg-state"))),
            PathBuf::from("/xdg-state/zeus")
        );
        assert_eq!(
            ZeusPaths::app_support_from("/home/user", None, None),
            PathBuf::from("/home/user/.local/state/zeus")
        );
        // A relative XDG_STATE_HOME is invalid and must be ignored.
        assert_eq!(
            ZeusPaths::app_support_from("/home/user", None, Some(Path::new("relative/state"))),
            PathBuf::from("/home/user/.local/state/zeus")
        );
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn macos_keeps_the_application_support_directory() {
        assert_eq!(
            ZeusPaths::app_support_from("/Users/user", None, Some(Path::new("/xdg-state"))),
            PathBuf::from("/Users/user/Library/Application Support/Zeus")
        );
    }
}
