//! Local enrollment and per-device credentials. No secret-bearing Debug impls.
use crate::config::{SecureDir, invalid};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    ffi::OsStr,
    fs::File,
    io,
    os::fd::AsRawFd,
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
struct Lock {
    file: File,
    directory: SecureDir,
}
impl Drop for Lock {
    fn drop(&mut self) {
        unsafe {
            libc::flock(self.file.as_raw_fd(), libc::LOCK_UN);
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
        let directory = SecureDir::open(&self.directory)?;
        let file = directory.open_file(OsStr::new("auth.lock"), libc::O_RDWR | libc::O_CREAT)?;
        // Never wait behind a stuck administrator or another gateway.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err(invalid("auth state busy"));
        }
        Ok(Lock { file, directory })
    }
    fn load(lock: &Lock) -> io::Result<Store> {
        let bytes = lock.directory.read(OsStr::new("devices.json"), MAX_STATE)?;
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
    fn save(lock: &Lock, s: &Store) -> io::Result<()> {
        let bytes = serde_json::to_vec(s).map_err(|_| invalid("auth serialization failed"))?;
        if bytes.len() > MAX_STATE {
            return Err(invalid("auth state full"));
        }
        lock.directory
            .atomic_write(OsStr::new("devices.json"), &bytes)
    }
    pub fn initialize(&self) -> io::Result<()> {
        let lock = self.lock()?;
        match lock
            .directory
            .open_file(OsStr::new("devices.json"), libc::O_RDONLY)
        {
            Ok(_) => return Err(invalid("auth state already exists")),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        Self::save(
            &lock,
            &Store {
                version: 1,
                server_id: random_token()?,
                ..Store::default()
            },
        )
    }
    pub fn server_id(&self) -> io::Result<String> {
        let lock = self.lock()?;
        Ok(Self::load(&lock)?.server_id)
    }
    pub fn enroll(&self, scopes: Vec<Scope>, now: u64) -> io::Result<String> {
        let lock = self.lock()?;
        let mut s = Self::load(&lock)?;
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
        Self::save(&lock, &s)?;
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
        let lock = self.lock()?;
        let mut s = Self::load(&lock)?;
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
        Self::save(&lock, &s)?;
        Ok(response)
    }
    pub fn authenticate(&self, token: &str, scope: Scope, now: u64) -> io::Result<Device> {
        if token.len() != 64 {
            return Err(invalid("unauthorized"));
        }
        let lock = self.lock()?;
        let s = Self::load(&lock)?;
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
        let lock = self.lock()?;
        Ok(Self::load(&lock)?
            .devices
            .into_iter()
            .map(|c| c.device)
            .collect())
    }
    pub fn revoke(&self, id: &str) -> io::Result<()> {
        let lock = self.lock()?;
        let mut s = Self::load(&lock)?;
        let device = s
            .devices
            .iter_mut()
            .find(|c| c.device.id == id)
            .ok_or_else(|| invalid("unknown device"))?;
        device.device.revoked = true;
        Self::save(&lock, &s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::create_secure_dir;

    #[test]
    fn transaction_state_and_lock_keep_one_directory_identity_after_replacement() {
        let temp = tempfile::tempdir_in(std::fs::canonicalize("/tmp").unwrap()).unwrap();
        let directory = temp.path().join("state");
        create_secure_dir(&directory).unwrap();
        let auth = AuthStore {
            directory: directory.clone(),
        };
        auth.initialize().unwrap();
        let original_id = auth.server_id().unwrap();
        let lock = auth.lock().unwrap();
        let lock_inode = lock.file.metadata().unwrap();

        let moved = temp.path().join("moved");
        std::fs::rename(&directory, &moved).unwrap();
        create_secure_dir(&directory).unwrap();
        auth.initialize().unwrap();
        let replacement_id = auth.server_id().unwrap();
        assert_ne!(original_id, replacement_id);
        let moved_auth = AuthStore { directory: moved };
        assert!(
            moved_auth.lock().is_err(),
            "old directory is still exclusively locked"
        );
        let mut store = AuthStore::load(&lock).unwrap();
        assert_eq!(store.server_id, original_id);
        store.enrollments.push(Enrollment {
            digest: digest("test"),
            expires_at_ms: 2000,
            scopes: vec![Scope::Read],
        });
        AuthStore::save(&lock, &store).unwrap();
        assert_eq!(auth.server_id().unwrap(), replacement_id);
        drop(lock);

        let new_lock = moved_auth.lock().unwrap();
        use std::os::unix::fs::MetadataExt;
        assert_eq!(new_lock.file.metadata().unwrap().ino(), lock_inode.ino());
        let stored = AuthStore::load(&new_lock).unwrap();
        assert_eq!(stored.server_id, original_id);
        assert_eq!(stored.enrollments.len(), 1);
        assert!(
            AuthStore::load(&auth.lock().unwrap())
                .unwrap()
                .enrollments
                .is_empty()
        );
    }
}
