//! Companion-access configuration owned by the current GPUI desktop app.
//!
//! The Swift daemon still owns the listener. This module detects the Mac's
//! Tailscale identity and writes the same owner-only `remote.json` schema the
//! legacy Swift settings pane used.

use std::path::{Path, PathBuf};
use std::process::Command;

use diri_proto::RemoteConfig;
use serde::Deserialize;

pub const REMOTE_PORT: u16 = 48_620;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TailscaleEndpoint {
    /// Exact local interface address used by the daemon listener.
    pub bind_host: String,
    /// Stable MagicDNS name shown to and stored by the phone when available.
    pub display_host: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CompanionAccess {
    pub config: Option<RemoteConfig>,
    pub tailscale: Option<TailscaleEndpoint>,
}

impl CompanionAccess {
    pub fn load(home: &Path) -> Self {
        let config = RemoteConfig::load(diri_proto::paths::DirijorPaths::remote_config_file(home));
        Self {
            config,
            // Subprocess discovery belongs in the explicit enable/repair task,
            // never on GPUI's render thread. Existing configs remain reachable
            // through their exact bound Tailscale address.
            tailscale: None,
        }
    }

    pub fn is_configured(&self) -> bool {
        self.config.is_some()
    }

    pub fn endpoint_label(&self) -> String {
        let Some(config) = &self.config else {
            return "No endpoint configured".to_owned();
        };
        let host = self
            .tailscale
            .as_ref()
            .map(|endpoint| endpoint.display_host.as_str())
            .or(config.bind_host.as_deref())
            .unwrap_or("loopback only");
        format!("{host}:{}", config.port)
    }

    pub fn pairing_url(&self) -> Option<String> {
        let config = self.config.as_ref()?;
        let host = self
            .tailscale
            .as_ref()
            .map(|endpoint| endpoint.display_host.as_str())
            .or(config.bind_host.as_deref())?;
        Some(format!(
            "dirijor://{host}:{}?token={}",
            config.port, config.token
        ))
    }
}

/// Enables or repairs companion access. Existing tokens are preserved so a
/// Tailscale address change does not strand an already-paired phone.
pub fn enable(home: &Path) -> Result<CompanionAccess, String> {
    let tailscale = detect_tailscale().ok_or_else(|| {
        "Tailscale must be installed and connected before companion access can be enabled."
            .to_owned()
    })?;
    let path = diri_proto::paths::DirijorPaths::remote_config_file(home);
    let existing = RemoteConfig::load(&path);
    let token = existing
        .as_ref()
        .map(|config| config.token.clone())
        .filter(|token| !token.is_empty())
        .map_or_else(generate_token, Ok)?;
    let config = RemoteConfig {
        port: existing.as_ref().map_or(REMOTE_PORT, |config| config.port),
        bind_host: Some(tailscale.bind_host.clone()),
        token,
        forward_any_port: existing.and_then(|config| config.forward_any_port),
    };
    config
        .save(&path)
        .map_err(|error| format!("Could not save companion access: {error}"))?;
    Ok(CompanionAccess {
        config: Some(config),
        tailscale: Some(tailscale),
    })
}

pub fn disable(home: &Path) -> Result<CompanionAccess, String> {
    let path = diri_proto::paths::DirijorPaths::remote_config_file(home);
    match std::fs::remove_file(&path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("Could not disable companion access: {error}")),
    }
    Ok(CompanionAccess {
        config: None,
        tailscale: None,
    })
}

#[derive(Deserialize)]
struct TailscaleStatus {
    #[serde(rename = "Self")]
    self_node: Option<TailscaleNode>,
}

#[derive(Deserialize)]
struct TailscaleNode {
    #[serde(rename = "DNSName")]
    dns_name: Option<String>,
    #[serde(rename = "TailscaleIPs")]
    tailscale_ips: Option<Vec<String>>,
}

fn detect_tailscale() -> Option<TailscaleEndpoint> {
    let commands: [(PathBuf, &[&str]); 4] = [
        (
            PathBuf::from("/Applications/Tailscale.app/Contents/MacOS/Tailscale"),
            &["status", "--json"],
        ),
        (
            PathBuf::from("/opt/homebrew/bin/tailscale"),
            &["status", "--json"],
        ),
        (
            PathBuf::from("/usr/local/bin/tailscale"),
            &["status", "--json"],
        ),
        (
            PathBuf::from("/usr/bin/env"),
            &["tailscale", "status", "--json"],
        ),
    ];
    commands.into_iter().find_map(|(executable, arguments)| {
        let output = Command::new(executable).args(arguments).output().ok()?;
        output
            .status
            .success()
            .then(|| parse_tailscale_status(&output.stdout))?
    })
}

fn parse_tailscale_status(bytes: &[u8]) -> Option<TailscaleEndpoint> {
    let status: TailscaleStatus = serde_json::from_slice(bytes).ok()?;
    let node = status.self_node?;
    let bind_host = node
        .tailscale_ips?
        .into_iter()
        .find(|address| address.parse::<std::net::Ipv4Addr>().is_ok())?;
    let display_host = node
        .dns_name
        .map(|name| name.trim_end_matches('.').to_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| bind_host.clone());
    Some(TailscaleEndpoint {
        bind_host,
        display_host,
    })
}

fn generate_token() -> Result<String, String> {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes)
        .map_err(|error| format!("Could not generate a secure pairing token: {error}"))?;
    Ok(base64_url_unpadded(&bytes))
}

fn base64_url_unpadded(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut encoded = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let a = chunk[0];
        let b = chunk.get(1).copied();
        let c = chunk.get(2).copied();
        encoded.push(char::from(ALPHABET[usize::from(a >> 2)]));
        encoded.push(char::from(
            ALPHABET[usize::from(((a & 0x03) << 4) | b.unwrap_or(0) >> 4)],
        ));
        if let Some(b) = b {
            encoded.push(char::from(
                ALPHABET[usize::from(((b & 0x0f) << 2) | c.unwrap_or(0) >> 6)],
            ));
        }
        if let Some(c) = c {
            encoded.push(char::from(ALPHABET[usize::from(c & 0x3f)]));
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_magic_dns_and_ipv4() {
        let endpoint = parse_tailscale_status(
            br#"{
                "Self": {
                    "DNSName": "studio.example.ts.net.",
                    "TailscaleIPs": ["100.90.80.70", "fd7a:115c:a1e0::1"]
                }
            }"#,
        )
        .expect("endpoint");
        assert_eq!(endpoint.bind_host, "100.90.80.70");
        assert_eq!(endpoint.display_host, "studio.example.ts.net");
    }

    #[test]
    fn base64_url_token_is_unpadded_and_url_safe() {
        assert_eq!(base64_url_unpadded(&[0xfb, 0xff, 0xef]), "-__v");
        let token = generate_token().expect("secure token");
        assert_eq!(token.len(), 43);
        assert!(
            token
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
        );
    }

    #[test]
    fn pairing_link_prefers_magic_dns_and_matches_the_ios_parser() {
        let access = CompanionAccess {
            config: Some(RemoteConfig {
                port: REMOTE_PORT,
                bind_host: Some("100.90.80.70".into()),
                token: "secret-token".into(),
                forward_any_port: None,
            }),
            tailscale: Some(TailscaleEndpoint {
                bind_host: "100.90.80.70".into(),
                display_host: "studio.example.ts.net".into(),
            }),
        };
        assert_eq!(
            access.pairing_url().as_deref(),
            Some("dirijor://studio.example.ts.net:48620?token=secret-token")
        );
        assert_eq!(access.endpoint_label(), "studio.example.ts.net:48620");
    }
}
