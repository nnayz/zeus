use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};

use sha2::{Digest, Sha256};

use taskmaster_proto::remote_pty::PROTOCOL_MAJOR;

#[derive(Clone, Debug)]
pub struct StatePaths {
    pub root: PathBuf,
    pub sessions: PathBuf,
    pub launch_locks: PathBuf,
}

impl StatePaths {
    pub fn resolve() -> io::Result<Self> {
        let root = match std::env::var_os("TASKMASTER_REMOTE_STATE_DIR") {
            Some(path) => PathBuf::from(path),
            None => match std::env::var_os("XDG_STATE_HOME") {
                Some(path) => PathBuf::from(path).join("taskmaster"),
                None => required_absolute_home()?
                    .join(".local")
                    .join("state")
                    .join("taskmaster"),
            },
        };
        Self::from_root(root)
    }

    pub fn from_root(root: PathBuf) -> io::Result<Self> {
        require_absolute_normal_path(&root)?;
        let sessions = root.join("sessions");
        let launch_locks = root.join("launch-locks");
        ensure_private_dir(&root)?;
        ensure_private_dir(&sessions)?;
        ensure_private_dir(&launch_locks)?;
        Ok(Self {
            root,
            sessions,
            launch_locks,
        })
    }

    pub fn session(&self, session_id: &str) -> io::Result<SessionPaths> {
        validate_identifier(session_id)?;
        let root = self.sessions.join(session_id);
        let sockets = runtime_socket_root()?;
        let digest = Sha256::digest(session_id.as_bytes());
        let socket_name = digest[..16]
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        Ok(SessionPaths {
            // macOS sockaddr_un paths are only 104 bytes. A hashed filename
            // under a short owner-only runtime root is stable across SSH
            // Bridges without making Session IDs path-length dependent.
            socket: sockets.join(format!("{socket_name}.sock")),
            lock: root.join("holder.lock"),
            launch_lock: self.launch_locks.join(format!("{socket_name}.lock")),
            state: root.join("session.json"),
            auth: root.join("auth.sha256"),
            output: root.join("output.log"),
            diagnostics: root.join("holder.log"),
            holder_start: root.join("holder-start.json"),
            root,
        })
    }
}

pub fn helper_protocol_root() -> io::Result<PathBuf> {
    let root = required_absolute_home()?
        .join(".cache")
        .join("taskmaster")
        .join("bin")
        .join(format!("protocol-{PROTOCOL_MAJOR}"));
    require_absolute_normal_path(&root)?;
    Ok(root)
}

/// Slack between a socket root and the socket path: `/` plus the 32-hex
/// session digest plus `.sock`.
const SOCKET_NAME_BYTES: usize = 38;
/// macOS caps `sockaddr_un.sun_path` at 104 bytes including the NUL
/// terminator; keep the whole socket path under this budget with a margin.
const SOCKET_PATH_BUDGET: usize = 100;

fn socket_root_fits(root: &Path) -> bool {
    root.as_os_str()
        .as_bytes()
        .len()
        .saturating_add(SOCKET_NAME_BYTES)
        < SOCKET_PATH_BUDGET
}

fn runtime_socket_root() -> io::Result<PathBuf> {
    // SAFETY: getuid has no preconditions and does not mutate memory.
    let uid = unsafe { libc::getuid() };
    let temp = std::env::temp_dir();
    // macOS' shadow `TMPDIR` under `/var/folders` leaves little room once the
    // product name is spelled out, so fall back to a shorter owner-scoped
    // directory there instead of failing at bind time, mirroring the
    // short-namespace mapping used for overlong OpenSSH `ControlPath` values.
    let candidates = [
        std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .map(|path| path.join("taskmaster")),
        Some(temp.join(format!("taskmaster-remote-{uid}"))),
        Some(temp.join(format!("tmr-{uid}"))),
    ];
    let root = candidates
        .into_iter()
        .flatten()
        .find(|root| socket_root_fits(root))
        .ok_or_else(|| {
            io::Error::other("no runtime directory is short enough for the session socket path")
        })?;
    ensure_private_dir(&root)?;
    Ok(root)
}

#[derive(Clone, Debug)]
pub struct SessionPaths {
    pub root: PathBuf,
    pub socket: PathBuf,
    pub lock: PathBuf,
    pub launch_lock: PathBuf,
    pub state: PathBuf,
    pub auth: PathBuf,
    pub output: PathBuf,
    pub diagnostics: PathBuf,
    pub holder_start: PathBuf,
}

impl SessionPaths {
    pub fn ensure(&self) -> io::Result<()> {
        ensure_private_dir(&self.root)
    }
}

pub fn validate_identifier(value: &str) -> io::Result<()> {
    let valid = !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'));
    if valid {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "identifier must be 1..=128 ASCII alphanumeric, '.', '_' or '-' bytes",
        ))
    }
}

pub fn ensure_private_dir(path: &Path) -> io::Result<()> {
    require_absolute_normal_path(path)?;
    ensure_ancestors_without_symlinks(path)?;
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "state path is not a directory",
            ))
        }
        Ok(_) => {
            fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
            Ok(())
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let mut builder = DirBuilder::new();
            builder.mode(0o700);
            match builder.create(path) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    ensure_private_dir(path)
                }
                Err(error) => Err(error),
            }
        }
        Err(error) => Err(error),
    }
}

pub fn create_private_file(path: &Path) -> io::Result<File> {
    reject_symlink(path)?;
    OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(path)
}

pub fn open_private_file(path: &Path) -> io::Result<File> {
    reject_symlink(path)?;
    OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .mode(0o600)
        .open(path)
}

pub fn open_private_truncate(path: &Path) -> io::Result<File> {
    reject_symlink(path)?;
    OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(path)
}

pub fn reject_symlink(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "refusing to follow a symlink in remote state",
        )),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn required_absolute_home() -> io::Result<PathBuf> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "HOME is not set"))?;
    require_absolute_normal_path(&home)?;
    Ok(home)
}

fn require_absolute_normal_path(path: &Path) -> io::Result<()> {
    let valid = path.is_absolute()
        && path
            .components()
            .all(|component| !matches!(component, Component::ParentDir | Component::CurDir));
    if valid {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "state path must be absolute and normalized",
        ))
    }
}

fn ensure_ancestors_without_symlinks(path: &Path) -> io::Result<()> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    if parent == Path::new("/") {
        return Ok(());
    }
    if !parent.exists() {
        ensure_ancestors_without_symlinks(parent)?;
        let mut builder = DirBuilder::new();
        builder.mode(0o700);
        match builder.create(parent) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    }
    let metadata = fs::symlink_metadata(parent)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "state path ancestor is a symlink or non-directory",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    #[test]
    fn session_paths_reject_traversal_and_are_owner_only() {
        let temporary = tempfile::tempdir().expect("temp");
        let paths = StatePaths {
            root: temporary.path().join("state"),
            sessions: temporary.path().join("state/sessions"),
            launch_locks: temporary.path().join("state/launch-locks"),
        };
        ensure_private_dir(&temporary.path().join("state")).expect("root");
        ensure_private_dir(&paths.sessions).expect("sessions");
        ensure_private_dir(&paths.launch_locks).expect("launch locks");
        assert!(paths.session("../escape").is_err());
        let session = paths.session("session-1").expect("session");
        session.ensure().expect("session dir");
        assert_eq!(
            fs::metadata(&session.root)
                .expect("metadata")
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }

    #[test]
    fn symlinked_session_directories_are_refused() {
        let temporary = tempfile::tempdir().expect("temp");
        let outside = temporary.path().join("outside");
        fs::create_dir(&outside).expect("outside");
        let link = temporary.path().join("link");
        symlink(&outside, &link).expect("symlink");
        assert!(ensure_private_dir(&link).is_err());
    }

    #[test]
    fn socket_root_budget_leaves_room_for_the_hashed_socket_name() {
        assert!(socket_root_fits(Path::new("/run/user/1000/taskmaster")));
        let long = PathBuf::from(format!("/{}/taskmaster", "x".repeat(72)));
        assert!(!socket_root_fits(&long));
    }

    #[test]
    fn chosen_socket_root_can_bind_a_hashed_socket_name() {
        let root = runtime_socket_root().expect("socket root");
        let socket = root.join(format!("{}.sock", "0".repeat(32)));
        let listener = std::os::unix::net::UnixListener::bind(&socket)
            .expect("the chosen root must bind within SUN_LEN");
        drop(listener);
        let _ = fs::remove_file(&socket);
    }
}
