//! Local enrollment and per-device credentials. No secret-bearing Debug impls.
use crate::config::{atomic_write, invalid, secure_dir, secure_read, validate_file};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{File, OpenOptions},
    io,
    os::{fd::AsRawFd, unix::fs::OpenOptionsExt},
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};
use subtle::ConstantTimeEq;
use zeus_companion_api::{Device, PairResponse, Scope};

const MAX_DEVICES: usize = 64;
const MAX_ENROLLMENTS: usize = 8;
const MAX_STATE: usize = 64 * 1024;
#[derive(Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct Store {
    version: u32,
    server_id: String,
    devices: Vec<Credential>,
    enrollments: Vec<Enrollment>,
}
#[derive(Serialize, Deserialize)]
struct Credential {
    device: Device,
    digest: String,
}
#[derive(Serialize, Deserialize)]
struct Enrollment {
    digest: String,
    expires_at_ms: u64,
    scopes: Vec<Scope>,
}
#[derive(Clone)]
pub struct AuthStore {
    pub directory: PathBuf,
}
struct Lock(File);
impl Drop for Lock {
    fn drop(&mut self) {
        unsafe {
            libc::flock(self.0.as_raw_fd(), libc::LOCK_UN);
        }
    }
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
pub fn random_token() -> io::Result<String> {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).map_err(|_| invalid("random source unavailable"))?;
    Ok(hex(&bytes))
}
pub fn digest(secret: &str) -> String {
    hex(&Sha256::digest(secret.as_bytes()))
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn matches(secret: &str, expected: &str) -> bool {
    let hash = digest(secret);
    hash.as_bytes().ct_eq(expected.as_bytes()).into()
}
impl AuthStore {
    fn lock(&self) -> io::Result<Lock> {
        secure_dir(&self.directory)?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(self.directory.join("auth.lock"))?;
        validate_file(&file)?;
        // Never wait behind a stuck administrator or another gateway.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err(invalid("auth state busy"));
        }
        Ok(Lock(file))
    }
    fn load(&self) -> io::Result<Store> {
        let bytes = secure_read(&self.directory.join("devices.json"), MAX_STATE)?;
        let store: Store =
            serde_json::from_slice(&bytes).map_err(|_| invalid("invalid auth state"))?;
        if store.version != 1
            || store.devices.len() > MAX_DEVICES
            || store.enrollments.len() > MAX_ENROLLMENTS
        {
            return Err(invalid("unsupported auth state"));
        }
        Ok(store)
    }
    fn save(&self, s: &Store) -> io::Result<()> {
        let bytes = serde_json::to_vec(s).map_err(|_| invalid("auth serialization failed"))?;
        if bytes.len() > MAX_STATE {
            return Err(invalid("auth state full"));
        }
        atomic_write(&self.directory.join("devices.json"), &bytes)
    }
    pub fn initialize(&self) -> io::Result<()> {
        let _lock = self.lock()?;
        if self.directory.join("devices.json").try_exists()? {
            return Err(invalid("auth state already exists"));
        }
        self.save(&Store {
            version: 1,
            server_id: random_token()?,
            ..Store::default()
        })
    }
    pub fn server_id(&self) -> io::Result<String> {
        let _lock = self.lock()?;
        Ok(self.load()?.server_id)
    }
    pub fn enroll(&self, scopes: Vec<Scope>, now: u64) -> io::Result<String> {
        let _lock = self.lock()?;
        let mut s = self.load()?;
        s.enrollments.retain(|e| e.expires_at_ms > now);
        if scopes.is_empty() || scopes.len() > 4 || s.enrollments.len() >= MAX_ENROLLMENTS {
            return Err(invalid("invalid enrollment or enrollment limit"));
        }
        let code = random_token()?;
        s.enrollments.push(Enrollment {
            digest: digest(&code),
            expires_at_ms: now.saturating_add(300_000),
            scopes,
        });
        self.save(&s)?;
        Ok(code)
    }
    pub fn pair(
        &self,
        code: &str,
        name: &str,
        expected_server_id: &str,
        now: u64,
    ) -> io::Result<PairResponse> {
        if code.len() != 64
            || name.is_empty()
            || name.len() > 64
            || name.chars().any(char::is_control)
        {
            return Err(invalid("pairing denied"));
        }
        let _lock = self.lock()?;
        let mut s = self.load()?;
        if s.server_id != expected_server_id {
            return Err(invalid("wrong server"));
        }
        if s.devices.len() >= MAX_DEVICES {
            return Err(invalid("device limit"));
        }
        let index = s
            .enrollments
            .iter()
            .position(|e| e.expires_at_ms > now && matches(code, &e.digest))
            .ok_or_else(|| invalid("pairing denied"))?;
        let enrollment = s.enrollments.remove(index);
        let token = random_token()?;
        let device = Device {
            id: random_token()?,
            name: name.to_owned(),
            scopes: enrollment.scopes,
            expires_at_ms: now.saturating_add(30 * 24 * 60 * 60 * 1000),
            revoked: false,
        };
        let response = PairResponse {
            server_id: s.server_id.clone(),
            device_id: device.id.clone(),
            token: token.clone(),
            scopes: device.scopes.clone(),
            expires_at_ms: device.expires_at_ms,
        };
        s.devices.push(Credential {
            device,
            digest: digest(&token),
        });
        self.save(&s)?;
        Ok(response)
    }
    pub fn authenticate(&self, token: &str, scope: Scope, now: u64) -> io::Result<Device> {
        if token.len() != 64 {
            return Err(invalid("unauthorized"));
        }
        let _lock = self.lock()?;
        let s = self.load()?;
        s.devices
            .into_iter()
            .find(|c| {
                matches(token, &c.digest)
                    && !c.device.revoked
                    && c.device.expires_at_ms > now
                    && c.device.scopes.contains(&scope)
            })
            .map(|c| c.device)
            .ok_or_else(|| invalid("unauthorized"))
    }
    pub fn list(&self) -> io::Result<Vec<Device>> {
        let _lock = self.lock()?;
        Ok(self.load()?.devices.into_iter().map(|c| c.device).collect())
    }
    pub fn revoke(&self, id: &str) -> io::Result<()> {
        let _lock = self.lock()?;
        let mut s = self.load()?;
        let device = s
            .devices
            .iter_mut()
            .find(|c| c.device.id == id)
            .ok_or_else(|| invalid("unknown device"))?;
        device.device.revoked = true;
        self.save(&s)
    }
}
