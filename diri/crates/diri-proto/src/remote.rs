//! Companion-access configuration (`remote.json`) shared with the Swift daemon.

use std::fs;
use std::io::{self, Write as _};
use std::path::Path;

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt};

use serde::{Deserialize, Serialize};

/// TCP listener configuration consumed by `dirijord` at startup.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteConfig {
    pub port: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bind_host: Option<String>,
    pub token: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub forward_any_port: Option<bool>,
}

impl RemoteConfig {
    pub fn load(path: impl AsRef<Path>) -> Option<Self> {
        fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
    }

    /// Atomically writes the shared secret with owner-only permissions, matching
    /// `RemoteConfig.save` in the Swift protocol package.
    pub fn save(&self, path: impl AsRef<Path>) -> io::Result<()> {
        let path = path.as_ref();
        let parent = path.parent().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "remote config path has no parent",
            )
        })?;
        fs::create_dir_all(parent)?;

        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("remote.json");
        let temporary = parent.join(format!(".{file_name}.tmp-{}", std::process::id()));
        let mut data = serde_json::to_vec_pretty(self).map_err(io::Error::other)?;
        data.push(b'\n');

        let result = (|| {
            #[cfg(unix)]
            {
                // The token is never briefly world-readable: create the temp
                // file with its final mode rather than tightening it afterward.
                let mut file = fs::OpenOptions::new()
                    .create(true)
                    .truncate(true)
                    .write(true)
                    .mode(0o600)
                    .open(&temporary)?;
                file.write_all(&data)?;
                file.sync_all()?;
                fs::set_permissions(&temporary, fs::Permissions::from_mode(0o600))?;
            }
            #[cfg(not(unix))]
            fs::write(&temporary, data)?;
            fs::rename(&temporary, path)?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_swift_schema_and_legacy_files() {
        let current = br#"{
            "port": 48620,
            "bindHost": "100.101.102.103",
            "token": "secret",
            "forwardAnyPort": false
        }"#;
        let config: RemoteConfig = serde_json::from_slice(current).expect("current schema");
        assert_eq!(config.port, 48_620);
        assert_eq!(config.bind_host.as_deref(), Some("100.101.102.103"));
        assert_eq!(config.token, "secret");
        assert_eq!(config.forward_any_port, Some(false));

        let legacy = br#"{"port":48620,"token":"secret"}"#;
        let config: RemoteConfig = serde_json::from_slice(legacy).expect("legacy schema");
        assert_eq!(config.bind_host, None);
        assert_eq!(config.forward_any_port, None);
    }

    #[test]
    fn save_is_reloadable_and_owner_only() {
        let directory = tempfile::tempdir().expect("temp directory");
        let path = directory.path().join("nested/remote.json");
        let config = RemoteConfig {
            port: 48_620,
            bind_host: Some("100.64.0.1".into()),
            token: "secret".into(),
            forward_any_port: None,
        };

        config.save(&path).expect("save config");
        assert_eq!(RemoteConfig::load(&path), Some(config));

        #[cfg(unix)]
        assert_eq!(
            fs::metadata(path).expect("metadata").permissions().mode() & 0o777,
            0o600
        );
    }
}
