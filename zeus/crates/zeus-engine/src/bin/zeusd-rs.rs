//! zeusd-rs — the authoritative local Zeus Engine.
//!
//! It owns local and remote session orchestration. Remote phase-one spawning,
//! reconnect and adoption are implemented here; later remote hooks, MCP,
//! migration and resource features remain explicit non-goals rather than
//! reasons to delegate remote behavior to another daemon.

#[cfg(unix)]
use std::os::fd::AsRawFd;
#[cfg(unix)]
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::sync::atomic::AtomicBool;
#[cfg(unix)]
use std::sync::{Arc, Mutex};

#[cfg(unix)]
use zeus_engine::control::{ControlServer, InjectionConfig};
#[cfg(unix)]
use zeus_engine::detect::ManifestEngine;
#[cfg(unix)]
use zeus_engine::registry::Registry;
#[cfg(unix)]
use zeus_engine::session::HolderConfig;

#[cfg(not(unix))]
fn main() {
    eprintln!("zeusd-rs requires a unix platform");
    std::process::exit(64);
}

#[cfg(unix)]
fn main() {
    if zeus_engine::screenshot::run_worker_if_requested() {
        return;
    }

    // Stamp process start on stderr: captured into zeusd.boot.log by the
    // app's launcher, and our only visibility for pre-log failures.
    eprintln!(
        "zeusd-rs: process start pid={} build=zeus-engine-{}",
        std::process::id(),
        env!("CARGO_PKG_VERSION")
    );

    // The app launches us with launchd's generic SHELL and minimal PATH.
    // Normalize both from the user's account before any session snapshots the
    // inherited environment: wrapped agents must return to the user's actual
    // shell (fish/zsh/…), and that shell owns the current tool PATH.
    let user_shell = login_shell();
    // SAFETY: single-threaded startup, before any spawn.
    unsafe { std::env::set_var("SHELL", &user_shell) };
    if let Some(path) = login_path(&user_shell) {
        // SAFETY: single-threaded startup, before any spawn.
        unsafe { std::env::set_var("PATH", &path) };
    }

    let app_support = app_support_dir();
    for dir in ["logs", "holders", "inject", "bin"] {
        let _ = std::fs::create_dir_all(app_support.join(dir));
    }

    // Singleton guard: hold an exclusive lock for our lifetime so a second
    // daemon (a relaunching app whose probe raced) exits instead of stealing
    // the live daemon's socket and orphaning its PTYs. The fd leaks on
    // purpose — it must stay open until process exit.
    let lock_path = app_support.join("daemon.lock");
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)
        .unwrap_or_else(|error| {
            eprintln!("zeusd-rs: cannot open {}: {error}", lock_path.display());
            std::process::exit(1);
        });
    // SAFETY: flock on an owned fd; non-blocking probe.
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        eprintln!("zeusd-rs: another daemon owns the lock — exiting");
        std::process::exit(0);
    }
    std::mem::forget(lock);

    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.canonicalize().ok())
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."));

    let (engine, failed) = load_manifests(&exe_dir, &app_support);
    if !failed.is_empty() {
        eprintln!(
            "zeusd-rs: {} manifest file(s) failed to parse: {failed:?}",
            failed.len()
        );
    }
    let engine = Arc::new(engine);
    if engine.ids().is_empty() {
        // An empty catalog fails silently downstream: every agent would spawn
        // as a bare shell. Refuse loudly instead.
        eprintln!("zeusd-rs: no agent manifests found — refusing to start");
        std::process::exit(1);
    }

    let logs_dir = app_support.join("logs");
    let holder = HolderConfig {
        holders_dir: app_support.join("holders"),
        executable: holder_executable(&exe_dir),
    };

    let mut registry = Registry::new(Arc::clone(&engine), app_support.join("state.json"));
    match registry.load() {
        Ok(count) => eprintln!("zeusd-rs: loaded {count} session record(s)"),
        Err(error) => eprintln!("zeusd-rs: state load: {error}"),
    }
    let adopted = registry.restore(&holder, &logs_dir);
    eprintln!(
        "zeusd-rs: adopted {} live holder session(s): {adopted:?}",
        adopted.len()
    );
    let registry = Arc::new(Mutex::new(registry));

    // Stable CLI path under App Support: hooks, Codex notify, and zeus-mcp
    // all reference this absolute path. A cargo-built zeusd-rs does not sit
    // next to a `zeus` binary, so inventing `target/debug/zeus` makes every
    // MCP tools/list fail.
    let cli_path = install_cli_helpers(&exe_dir, &app_support);
    let mut server = ControlServer::new(Arc::clone(&registry), app_support.join("daemon.sock"))
        .with_logs_dir(&logs_dir)
        .with_holder(holder)
        .with_injection(InjectionConfig {
            inject_dir: app_support.join("inject"),
            cli_path,
        });
    if let Some(remote) = remote_manager(&exe_dir, &app_support) {
        server = server.with_remote(remote);
    }
    let server = Arc::new(server);
    let listener = match server.bind() {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("zeusd-rs: bind: {error}");
            // A live socket means a daemon is already serving; that is the
            // singleton working, not a failure.
            std::process::exit(if error.kind() == std::io::ErrorKind::AddrInUse {
                0
            } else {
                1
            });
        }
    };

    // Only once the socket is accepting: remote adoption is SSH-bound and must
    // never be what a client waits behind.
    server.spawn_remote_restore();

    let _watcher = zeus_engine::events::spawn_registry_watcher(
        Arc::clone(&registry),
        server.events(),
        Arc::new(AtomicBool::new(false)),
    );
    let pr_monitor_wake = server.pr_monitor_wake();
    let _governor = zeus_engine::governor::spawn_governor(
        Arc::clone(&registry),
        server.events(),
        server.attach_hub(),
        pr_monitor_wake.clone(),
        server.governor_config(),
        Arc::new(AtomicBool::new(false)),
    );
    let _pr_monitor = zeus_engine::pr_monitor::spawn_pr_monitor(
        Arc::clone(&registry),
        server.events(),
        server.attach_hub(),
        pr_monitor_wake,
        Arc::new(AtomicBool::new(false)),
    );
    let _persist_flusher = zeus_engine::registry::spawn_persist_flusher(
        Arc::clone(&registry),
        Arc::new(AtomicBool::new(false)),
    );

    eprintln!("zeusd-rs: serving {}", server.socket_path().display());
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let server = Arc::clone(&server);
                let _ = std::thread::Builder::new()
                    .name("zeusd-connection".into())
                    .spawn(move || {
                        let _ = server.serve(stream);
                    });
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => {
                eprintln!("zeusd-rs: accept: {error}");
                break;
            }
        }
    }
}

/// The user's real login shell from the user database. Authoritative even
/// under launchd, where the SHELL env var is often /bin/zsh regardless of the
/// user's configured shell (a fish user's PATH lives in config.fish, which
/// zsh would never source).
#[cfg(unix)]
fn login_shell() -> String {
    // SAFETY: getpwuid returns a pointer to a static per-thread record; it is
    // read immediately and never retained.
    unsafe {
        let record = libc::getpwuid(libc::getuid());
        if !record.is_null() {
            let shell = std::ffi::CStr::from_ptr((*record).pw_shell);
            if let Ok(shell) = shell.to_str()
                && !shell.is_empty()
                && Path::new(shell).exists()
            {
                return shell.to_owned();
            }
        }
    }
    std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into())
}

/// Mirrors the Swift daemon's `LoginEnvironment`: `printenv PATH` prints the
/// real colon-separated variable regardless of shell — fish stores $PATH as a
/// space-separated list, so `echo $PATH` produces garbage there — and `-i -l`
/// sources both interactive and login files, which is where agent PATHs are
/// actually configured.
///
/// Hard ceiling: wait for the shell to exit, then read stdout. On timeout,
/// SIGKILL the process group (not SIGTERM — rc files can trap that) and fall
/// back. Never block on an unbounded pipe read while the writer may still live.
#[cfg(unix)]
fn login_path(shell: &str) -> Option<String> {
    login_path_with_timeout(shell, std::time::Duration::from_secs(5))
}

#[cfg(unix)]
fn login_path_with_timeout(shell: &str, capture_timeout: std::time::Duration) -> Option<String> {
    capture_login_path(shell, &["-i", "-l", "-c", "printenv PATH"], capture_timeout)
}

#[cfg(unix)]
fn capture_login_path(
    shell: &str,
    arguments: &[&str],
    capture_timeout: std::time::Duration,
) -> Option<String> {
    use std::io::{Read, Seek};
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    let fallback = || {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
        format!("{home}/.local/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin")
    };

    // A background process from an rc file can inherit stdout after its shell
    // exits. Capturing into an unlinked regular file means reading stops at the
    // current length instead of waiting for that descendant to close a pipe.
    let mut capture = anonymous_capture_file().ok()?;
    let child_stdout = capture.try_clone().ok()?;
    let mut child = unsafe {
        Command::new(shell)
            .args(arguments)
            .stdout(Stdio::from(child_stdout))
            .stderr(Stdio::null())
            .pre_exec(|| {
                // Own process group so trapped shells / hung children die with us.
                if libc::setpgid(0, 0) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            })
            .spawn()
    }
    .ok()?;

    let started = Instant::now();
    let timed_out = loop {
        match child.try_wait() {
            Ok(Some(_)) => break false,
            Ok(None) if started.elapsed() < capture_timeout => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Ok(None) | Err(_) => break true,
        }
    };

    if timed_out {
        let pid = child.id() as i32;
        // SAFETY: pid is this child's id; negative targets its process group.
        unsafe {
            let _ = libc::kill(-pid, libc::SIGKILL);
            let _ = libc::kill(pid, libc::SIGKILL);
        }
        let _ = child.wait();
        return Some(fallback());
    }

    capture.rewind().ok()?;
    let mut bytes = Vec::new();
    let _ = capture.take(1 << 20).read_to_end(&mut bytes);
    let stdout = String::from_utf8_lossy(&bytes);
    // Interactive shells may print a greeting; take the last line that looks
    // like a PATH.
    let path = stdout
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| line.contains('/'))
        .map(str::to_owned)?;
    if path.is_empty() {
        return Some(fallback());
    }
    // A single-entry answer smells like a broken profile: keep it, but append
    // the standard locations so spawns still work.
    Some(if path.contains(':') {
        path
    } else {
        format!("{path}:{}", fallback())
    })
}

#[cfg(unix)]
fn anonymous_capture_file() -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;

    for _ in 0..8 {
        let mut nonce = [0_u8; 8];
        getrandom::fill(&mut nonce).map_err(|error| std::io::Error::other(error.to_string()))?;
        let suffix = nonce
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let path = std::env::temp_dir().join(format!("zeus-path-{}-{suffix}", std::process::id()));
        match std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
        {
            Ok(file) => {
                std::fs::remove_file(path)?;
                return Ok(file);
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "could not allocate a unique PATH capture file",
    ))
}

#[cfg(unix)]
fn app_support_dir() -> PathBuf {
    if let Ok(root) = std::env::var("ZEUS_APP_SUPPORT") {
        return PathBuf::from(root);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    Path::new(&home).join("Library/Application Support/Zeus")
}

/// Copy `zeus`, `zeus-mcp`, and the Agent catalog into App Support `bin/`,
/// then return the stable `zeus` path used for injection.
#[cfg(unix)]
fn install_cli_helpers(exe_dir: &Path, app_support: &Path) -> PathBuf {
    let bin_dir = app_support.join("bin");
    let _ = std::fs::create_dir_all(&bin_dir);
    for name in ["zeus", "zeus-mcp"] {
        let dest = bin_dir.join(name);
        let Some(source) = cli_helper_sources(exe_dir, name)
            .into_iter()
            .find(|path| is_executable(path))
        else {
            continue;
        };
        if source.canonicalize().ok() == dest.canonicalize().ok() {
            continue;
        }
        match install_cli_helper(&source, &dest) {
            Ok(()) => eprintln!(
                "zeusd-rs: installed helper: {} -> {}",
                source.display(),
                dest.display()
            ),
            Err(error) => eprintln!(
                "zeusd-rs: helper install failed for {name}: {error} (source {})",
                source.display()
            ),
        }
    }
    install_cli_manifests(exe_dir, &bin_dir);
    let stable = bin_dir.join("zeus");
    if is_executable(&stable) {
        stable
    } else if is_executable(&exe_dir.join("zeus")) {
        exe_dir.join("zeus")
    } else {
        // Last resort: PATH lookup at spawn time. Still better than a path
        // that is known not to exist beside this Engine binary.
        PathBuf::from("zeus")
    }
}

#[cfg(unix)]
fn install_cli_helper(source: &Path, dest: &Path) -> std::io::Result<()> {
    let file_name = dest
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid helper name")
        })?;
    let staging = dest.with_file_name(format!(".{file_name}.{}.tmp", std::process::id()));
    let _ = std::fs::remove_file(&staging);
    std::fs::copy(source, &staging)?;
    set_executable(&staging);
    if let Err(error) = std::fs::rename(&staging, dest) {
        let _ = std::fs::remove_file(&staging);
        return Err(error);
    }
    Ok(())
}

#[cfg(unix)]
fn install_cli_manifests(exe_dir: &Path, bin_dir: &Path) {
    const NAME: &str = "manifests";
    let Some(source) = cli_manifest_sources(exe_dir)
        .into_iter()
        .find(|path| path.is_dir())
    else {
        return;
    };
    let dest = bin_dir.join(NAME);
    if source.canonicalize().ok() == dest.canonicalize().ok() {
        return;
    }
    let staging = bin_dir.join(format!(".{NAME}.{}.tmp", std::process::id()));
    let _ = std::fs::remove_dir_all(&staging);
    if let Err(error) = copy_dir(&source, &staging) {
        let _ = std::fs::remove_dir_all(&staging);
        eprintln!(
            "zeusd-rs: helper resource install failed: {error} (source {})",
            source.display()
        );
        return;
    }
    let _ = std::fs::remove_dir_all(&dest);
    if let Err(error) = std::fs::rename(&staging, &dest) {
        let _ = std::fs::remove_dir_all(&staging);
        eprintln!("zeusd-rs: helper resource activation failed: {error}");
    }
}

#[cfg(unix)]
fn copy_dir(source: &Path, dest: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dest)?;
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let target = dest.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else if file_type.is_file() {
            std::fs::copy(entry.path(), target)?;
        } else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "resource bundle contains a symlink or special file",
            ));
        }
    }
    Ok(())
}

#[cfg(unix)]
fn cli_helper_sources(exe_dir: &Path, name: &str) -> Vec<PathBuf> {
    let mut sources = Vec::new();
    if name == "zeus" {
        // A loose Cargo build puts the desktop app at `target/*/zeus` and the
        // automation CLI at `target/*/zeus-cli`. The stable helper must be the
        // latter: zeus-mcp invokes it with `mcp-tools` / `mcp-call`, while the
        // desktop binary enters its GPUI event loop and leaves `tools/list`
        // unanswered. Packaged layouts name the CLI itself `zeus`, so the
        // ordinary sibling remains the fallback immediately below.
        sources.push(exe_dir.join("zeus-cli"));
    }
    sources.push(exe_dir.join(name));
    if let Ok(home) = std::env::var("HOME") {
        sources.push(
            Path::new(&home)
                .join("Applications/zeus.app/Contents/Resources/bin")
                .join(name),
        );
    }
    sources.push(PathBuf::from("/Applications/zeus.app/Contents/Resources/bin").join(name));
    sources
}

#[cfg(unix)]
fn cli_manifest_sources(exe_dir: &Path) -> Vec<PathBuf> {
    let mut sources = vec![exe_dir.join("manifests")];
    if let Some(workspace) = exe_dir.parent().and_then(Path::parent) {
        sources.push(workspace.join("crates/zeus-engine/manifests"));
    }
    if let Ok(home) = std::env::var("HOME") {
        sources
            .push(Path::new(&home).join("Applications/zeus.app/Contents/Resources/bin/manifests"));
    }
    sources.push(PathBuf::from(
        "/Applications/zeus.app/Contents/Resources/bin/manifests",
    ));
    sources
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(unix)]
fn set_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(meta) = std::fs::metadata(path) {
        let mut perms = meta.permissions();
        perms.set_mode(perms.mode() | 0o755);
        let _ = std::fs::set_permissions(path, perms);
    }
}

#[cfg(unix)]
/// Rust-owned base catalog next to the executable, then user overrides, then
/// the source-tree fallback used by loose development binaries.
fn load_manifests(exe_dir: &Path, app_support: &Path) -> (ManifestEngine, Vec<String>) {
    let mut bases: Vec<PathBuf> = Vec::new();
    if let Ok(configured) = std::env::var("ZEUS_MANIFESTS_DIR") {
        bases.push(PathBuf::from(configured));
    }
    bases.push(exe_dir.join("manifests"));
    bases.push(zeus_engine::detect::bundled_manifest_dir());
    let base = bases.into_iter().find(|dir| dir.is_dir());
    let overrides = app_support.join("manifests/overrides");

    let mut dirs: Vec<&Path> = Vec::new();
    if let Some(base) = &base {
        dirs.push(base);
    }
    dirs.push(&overrides);
    ManifestEngine::load_dirs(&dirs).unwrap_or_else(|error| {
        eprintln!("zeusd-rs: manifest load: {error}");
        (ManifestEngine::new(Vec::new()), Vec::new())
    })
}

#[cfg(unix)]
fn holder_executable(exe_dir: &Path) -> PathBuf {
    exe_dir.join("zeus-holder")
}

#[cfg(unix)]
fn remote_manager(
    exe_dir: &Path,
    app_support: &Path,
) -> Option<Arc<zeus_engine::remote::manager::RemoteManager>> {
    use zeus_engine::remote::executor::ProcessExecutor;
    use zeus_engine::remote::manager::{ArtifactCatalog, RemoteManager};

    let configured = std::env::var_os("ZEUS_REMOTE_HELPER_PATH").map(PathBuf::from);
    let Some(source) = resolve_remote_catalog_source(exe_dir, configured.as_deref()) else {
        eprintln!("zeusd-rs: remote transport disabled: no current Helper artifact");
        return None;
    };
    let catalog = match source {
        RemoteCatalogSource::Native(path) => ArtifactCatalog::from_native_helper(&path),
        RemoteCatalogSource::Manifest(path) => ArtifactCatalog::from_manifest(&path),
    };
    let catalog = match catalog {
        Ok(catalog) => catalog,
        Err(error) => {
            eprintln!("zeusd-rs: remote Helper catalog rejected: {error}");
            return None;
        }
    };
    let askpass = exe_dir.join("zeus-ssh-askpass");
    let executor = if askpass.is_file() {
        ProcessExecutor::default().with_askpass(askpass.into_os_string())
    } else {
        eprintln!(
            "zeusd-rs: SSH UI broker is unavailable at {}; interactive authentication is disabled",
            askpass.display()
        );
        ProcessExecutor::default()
    };
    match RemoteManager::new(executor, catalog, app_support.join("ssh-control")) {
        Ok(manager) => Some(Arc::new(manager)),
        Err(error) => {
            eprintln!("zeusd-rs: remote manager initialization failed: {error}");
            None
        }
    }
}

#[cfg(unix)]
#[derive(Clone, Debug, Eq, PartialEq)]
enum RemoteCatalogSource {
    Native(PathBuf),
    Manifest(PathBuf),
}

/// Loose Cargo builds place the just-built native Helper beside the Engine,
/// while packaged apps contain only the cross-platform manifest. Prefer the
/// sibling in the former layout so an old `target/remote-helpers` directory
/// can never silently define the current development build.
#[cfg(unix)]
fn resolve_remote_catalog_source(
    exe_dir: &Path,
    configured: Option<&Path>,
) -> Option<RemoteCatalogSource> {
    if let Some(path) = configured {
        return Some(RemoteCatalogSource::Native(path.to_path_buf()));
    }
    let sibling = exe_dir.join("zeus-remote");
    if sibling.is_file() {
        return Some(RemoteCatalogSource::Native(sibling));
    }
    [
        exe_dir.join("remote-helpers/manifest.json"),
        exe_dir.join("zeus-remote-helpers/manifest.json"),
    ]
    .into_iter()
    .find(|path| path.is_file())
    .map(RemoteCatalogSource::Manifest)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn login_path_capture_does_not_wait_for_a_child_that_inherited_stdout() {
        use std::time::{Duration, Instant};

        let started = Instant::now();
        let path = capture_login_path(
            "/bin/sh",
            &["-c", "/bin/sleep 5 & printf '/fixture:/usr/bin\\n'"],
            Duration::from_secs(2),
        );

        assert_eq!(path.as_deref(), Some("/fixture:/usr/bin"));
        assert!(started.elapsed() < Duration::from_secs(3));
    }

    #[test]
    fn login_path_capture_kills_a_shell_that_exceeds_the_deadline() {
        use std::time::{Duration, Instant};

        let started = Instant::now();
        let path = capture_login_path(
            "/bin/sh",
            &["-c", "/bin/sleep 5; printf '/too-late:/usr/bin\\n'"],
            Duration::from_millis(500),
        );

        assert_ne!(path.as_deref(), Some("/too-late:/usr/bin"));
        assert!(started.elapsed() < Duration::from_secs(3));
    }

    #[test]
    fn cli_helper_replacement_keeps_the_running_inode_intact() {
        use std::io::Read;

        let temporary = tempfile::tempdir().expect("temp");
        let source = temporary.path().join("source");
        let dest = temporary.path().join("zeus-mcp");
        std::fs::write(&source, b"new").expect("source");
        std::fs::write(&dest, b"old").expect("dest");
        let mut running = std::fs::File::open(&dest).expect("running helper");

        install_cli_helper(&source, &dest).expect("install");

        let mut old = String::new();
        running.read_to_string(&mut old).expect("old inode");
        assert_eq!(old, "old");
        assert_eq!(std::fs::read_to_string(&dest).expect("new path"), "new");
    }

    #[test]
    fn cli_helper_install_prefers_cargo_cli_over_sibling_desktop_app() {
        let temporary = tempfile::tempdir().expect("temp");
        let executables = temporary.path().join("target/debug");
        let app_support = temporary.path().join("app-support");
        std::fs::create_dir_all(&executables).expect("executables");

        let desktop = executables.join("zeus");
        let cli = executables.join("zeus-cli");
        std::fs::write(&desktop, b"desktop-app").expect("desktop");
        std::fs::write(&cli, b"automation-cli").expect("cli");
        set_executable(&desktop);
        set_executable(&cli);

        let installed = install_cli_helpers(&executables, &app_support);

        assert_eq!(installed, app_support.join("bin/zeus"));
        assert_eq!(
            std::fs::read(installed).expect("installed helper"),
            b"automation-cli"
        );
    }

    #[test]
    fn cli_manifests_are_installed_without_stale_files() {
        let temporary = tempfile::tempdir().expect("temp");
        let source = temporary.path().join("source");
        let bin = temporary.path().join("bin");
        let manifests = source.join("manifests");
        std::fs::create_dir_all(&manifests).expect("source catalog");
        std::fs::write(manifests.join("cursor.json"), b"cursor").expect("cursor manifest");

        install_cli_manifests(&source, &bin);
        let installed = bin.join("manifests");
        assert_eq!(
            std::fs::read(installed.join("cursor.json")).expect("installed cursor manifest"),
            b"cursor"
        );

        std::fs::remove_file(manifests.join("cursor.json")).expect("remove old manifest");
        std::fs::write(manifests.join("codex.json"), b"codex").expect("codex manifest");
        install_cli_manifests(&source, &bin);
        assert!(!installed.join("cursor.json").exists());
        assert!(installed.join("codex.json").exists());
    }

    #[test]
    fn loose_build_prefers_current_sibling_over_a_stale_catalog() {
        let temporary = tempfile::tempdir().expect("temp");
        let sibling = temporary.path().join("zeus-remote");
        let stale = temporary.path().join("remote-helpers/manifest.json");
        std::fs::create_dir_all(stale.parent().expect("manifest parent")).expect("catalog dir");
        std::fs::write(&sibling, b"current").expect("sibling");
        std::fs::write(&stale, b"stale").expect("manifest");

        assert_eq!(
            resolve_remote_catalog_source(temporary.path(), None),
            Some(RemoteCatalogSource::Native(sibling))
        );
    }

    #[test]
    fn packaged_layout_uses_the_cross_platform_manifest() {
        let temporary = tempfile::tempdir().expect("temp");
        let manifest = temporary.path().join("remote-helpers/manifest.json");
        std::fs::create_dir_all(manifest.parent().expect("manifest parent")).expect("catalog dir");
        std::fs::write(&manifest, b"catalog").expect("manifest");

        assert_eq!(
            resolve_remote_catalog_source(temporary.path(), None),
            Some(RemoteCatalogSource::Manifest(manifest))
        );
    }
}
