//! Launch the Swift daemon (`dirijord`) that ships bundled inside `diri.app`.
//!
//! diri talks to the existing Dirijor daemon over a Unix socket
//! (`~/Library/Application Support/Dirijor/daemon.sock`). Historically the
//! legacy `Dirijor.app` was responsible for spawning that daemon; now that diri
//! is self-contained it must launch the daemon itself when no live one exists.
//!
//! This is intentionally *launch-only* during ordinary app lifecycle: we never
//! compare `executableHash` or restart automatically. The one exception is an
//! explicit companion-access change in Settings, where the user has asked the
//! daemon to reload `remote.json`. The daemon holds an `flock` singleton, so a
//! redundant spawn still exits instantly.

use std::io;
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use diri_proto::paths::{DirijorPaths, ENV_SOCKET};

/// Environment override pointing directly at a `dirijord` executable, mirroring
/// the Swift `DIRIJOR_HOLDER_PATH` seam used by `HolderLauncher`.
const ENV_DAEMON_PATH: &str = "DIRIJORD_PATH";

const BOOT_LOG_FILE_NAME: &str = "dirijord.boot.log";

/// Ensure a daemon is reachable at `socket_path`, spawning the bundled
/// `dirijord` detached if the socket is dead.
///
/// Non-blocking: after a spawn we return immediately and let
/// [`diri_client::DaemonClient`]'s own reconnect loop (500 ms → 8 s backoff)
/// connect once the daemon's socket comes up. The UI is never blocked on
/// daemon startup.
pub fn ensure_daemon_running(socket_path: &Path) {
    // A dev/test harness that manages its own daemon exports DIRIJOR_SOCKET;
    // never spawn on top of it.
    if std::env::var_os(ENV_SOCKET).is_some() {
        return;
    }

    if socket_is_live(socket_path) {
        return;
    }

    match resolve_daemon_path() {
        Some(daemon) => match spawn_detached(&daemon) {
            Ok(()) => eprintln!("diri: launched bundled daemon at {}", daemon.display()),
            Err(err) => {
                eprintln!(
                    "diri: failed to launch bundled daemon {}: {err}",
                    daemon.display()
                );
            }
        },
        None => eprintln!(
            "diri: no bundled dirijord found next to the executable; \
             relying on an externally managed daemon"
        ),
    }
}

/// Waits for a user-requested daemon shutdown to release its socket, then
/// launches the bundled daemon if launchd or the legacy app has not already done
/// so. This must run off the UI thread because the bounded wait is synchronous.
pub fn relaunch_after_remote_config_change(socket_path: &Path) {
    for _ in 0..30 {
        if !socket_is_live(socket_path) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    ensure_daemon_running(socket_path);
}

/// True when something is listening on the daemon socket right now.
fn socket_is_live(socket_path: &Path) -> bool {
    UnixStream::connect(socket_path).is_ok()
}

/// Resolve the `dirijord` executable to launch, using the live process layout.
pub fn resolve_daemon_path() -> Option<PathBuf> {
    resolve_daemon_path_from(
        std::env::var_os(ENV_DAEMON_PATH).map(PathBuf::from),
        std::env::current_exe().ok(),
        std::env::current_dir().ok(),
    )
}

/// Pure resolver, split out so the bundle layout can be unit-tested without a
/// real `diri.app`.
///
/// Search order (first executable wins), mirroring
/// `HolderLauncher.defaultExecutablePath`:
///   1. `DIRIJORD_PATH` override (dev/tests).
///   2. Bundled: `Contents/MacOS/diri` → `../Resources/bin/dirijord`.
///   3. Next to the executable (loose dev copy).
///   4. Swift SPM build outputs under the working dir: `.build/{release,debug}/dirijord`.
fn resolve_daemon_path_from(
    env_override: Option<PathBuf>,
    current_exe: Option<PathBuf>,
    current_dir: Option<PathBuf>,
) -> Option<PathBuf> {
    if let Some(path) = env_override
        && is_executable(&path)
    {
        return Some(path);
    }

    let mut candidates: Vec<PathBuf> = Vec::new();

    if let Some(exe) = current_exe
        && let Some(macos_dir) = exe.parent()
    {
        // Contents/MacOS/diri → Contents/Resources/bin/dirijord
        if let Some(contents) = macos_dir.parent() {
            candidates.push(contents.join("Resources/bin/dirijord"));
        }
        // Loose copy sitting right next to the executable.
        candidates.push(macos_dir.join("dirijord"));
    }

    if let Some(cwd) = current_dir {
        candidates.push(cwd.join(".build/release/dirijord"));
        candidates.push(cwd.join(".build/debug/dirijord"));
    }

    candidates.into_iter().find(|path| is_executable(path))
}

/// Spawn `dirijord` in its own process group so it outlives diri, with
/// stdout/stderr appended to `dirijord.boot.log` (the same boot log the legacy
/// Swift app used — our only window into pre-`DaemonLog` failures). We never
/// wait on the child: the daemon is meant to run independently.
fn spawn_detached(daemon: &Path) -> io::Result<()> {
    let mut command = Command::new(daemon);
    command.stdin(Stdio::null());

    match boot_log_path() {
        Some(log_path) => {
            let out = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&log_path)?;
            let err = out.try_clone()?;
            command.stdout(Stdio::from(out)).stderr(Stdio::from(err));
        }
        None => {
            command.stdout(Stdio::null()).stderr(Stdio::null());
        }
    }

    // New process group (setpgid to the child's own pid): decouples the daemon
    // from diri's signal/terminal group so quitting diri never SIGHUPs the
    // daemon or its PTYs. Equivalent intent to the Swift POSIX_SPAWN_SETSID path.
    command.process_group(0);

    // Spawn and deliberately drop the handle — we do not (and must not) wait.
    command.spawn().map(|_child| ())
}

/// `~/Library/Application Support/Dirijor/dirijord.boot.log`, creating the
/// support directory if needed. Returns `None` when `HOME` is unset.
fn boot_log_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    let support = DirijorPaths::app_support(PathBuf::from(home));
    std::fs::create_dir_all(&support).ok()?;
    Some(support.join(BOOT_LOG_FILE_NAME))
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn touch_executable(path: &Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"#!/bin/sh\n").unwrap();
        let mut perms = std::fs::metadata(path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(path, perms).unwrap();
    }

    #[test]
    fn resolves_bundled_daemon_from_contents_macos() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let exe = root.join("Contents/MacOS/diri");
        touch_executable(&exe);
        let daemon = root.join("Contents/Resources/bin/dirijord");
        touch_executable(&daemon);

        let resolved =
            resolve_daemon_path_from(None, Some(exe), None).expect("bundled daemon should resolve");
        assert_eq!(
            std::fs::canonicalize(resolved).unwrap(),
            std::fs::canonicalize(daemon).unwrap(),
        );
    }

    #[test]
    fn env_override_wins_when_executable() {
        let tmp = tempfile::tempdir().unwrap();
        let override_path = tmp.path().join("custom/dirijord");
        touch_executable(&override_path);

        let resolved = resolve_daemon_path_from(Some(override_path.clone()), None, None).unwrap();
        assert_eq!(resolved, override_path);
    }

    #[test]
    fn ignores_non_executable_override_and_falls_back_next_to_exe() {
        let tmp = tempfile::tempdir().unwrap();
        // A non-executable override must be skipped.
        let bad_override = tmp.path().join("not-exec");
        std::fs::write(&bad_override, b"plain").unwrap();

        let exe = tmp.path().join("bin/diri");
        touch_executable(&exe);
        let sibling = tmp.path().join("bin/dirijord");
        touch_executable(&sibling);

        let resolved = resolve_daemon_path_from(Some(bad_override), Some(exe), None).unwrap();
        assert_eq!(
            std::fs::canonicalize(resolved).unwrap(),
            std::fs::canonicalize(sibling).unwrap(),
        );
    }

    #[test]
    fn returns_none_when_nothing_resolves() {
        let tmp = tempfile::tempdir().unwrap();
        let exe = tmp.path().join("Contents/MacOS/diri");
        touch_executable(&exe);
        // No daemon anywhere; cwd points at an empty dir.
        assert!(
            resolve_daemon_path_from(None, Some(exe), Some(tmp.path().to_path_buf())).is_none()
        );
    }
}
