//! dirijord-rs — the Rust Dirijor daemon.
//!
//! Drop-in for the Swift `dirijord`: same socket, same on-disk state, same
//! wire protocol, same holder adoption. Point the app at it with
//! `DIRIJORD_PATH` (the launch override `daemon_launch.rs` already honors) to
//! opt a machine in; live sessions carry over because both daemons speak the
//! same holder protocol.
//!
//! Known gaps vs the Swift daemon, all answering clean `not_found`s:
//! session.migrate, host.*, test.run (browser pool), remote-host spawning,
//! mobile ownership arbitration, resource sampling.

#[cfg(unix)]
use std::os::fd::AsRawFd;
#[cfg(unix)]
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::sync::atomic::AtomicBool;
#[cfg(unix)]
use std::sync::{Arc, Mutex};

#[cfg(unix)]
use diri_engine::control::{ControlServer, InjectionConfig};
#[cfg(unix)]
use diri_engine::detect::ManifestEngine;
#[cfg(unix)]
use diri_engine::registry::Registry;
#[cfg(unix)]
use diri_engine::session::HolderConfig;

#[cfg(not(unix))]
fn main() {
    eprintln!("dirijord-rs requires a unix platform");
    std::process::exit(64);
}

#[cfg(unix)]
fn main() {
    // Stamp process start on stderr: captured into dirijord.boot.log by the
    // app's launcher, and our only visibility for pre-log failures.
    eprintln!(
        "dirijord-rs: process start pid={} build=diri-engine-{}",
        std::process::id(),
        env!("CARGO_PKG_VERSION")
    );

    // The app launches us with launchd's minimal PATH; agents and tools
    // (claude, gh, node) live in the user's login PATH. Resolve it the way
    // the Swift daemon's LoginEnvironment did: ask the login shell once.
    if let Some(path) = login_path() {
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
            eprintln!("dirijord-rs: cannot open {}: {error}", lock_path.display());
            std::process::exit(1);
        });
    // SAFETY: flock on an owned fd; non-blocking probe.
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        eprintln!("dirijord-rs: another daemon owns the lock — exiting");
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
        eprintln!("dirijord-rs: {} manifest file(s) failed to parse: {failed:?}", failed.len());
    }
    let engine = Arc::new(engine);
    if engine.ids().is_empty() {
        // An empty catalog fails silently downstream: every agent would spawn
        // as a bare shell. Refuse loudly instead.
        eprintln!("dirijord-rs: no agent manifests found — refusing to start");
        std::process::exit(1);
    }

    let logs_dir = app_support.join("logs");
    let holder = HolderConfig {
        holders_dir: app_support.join("holders"),
        executable: holder_executable(&exe_dir),
    };

    let mut registry = Registry::new(Arc::clone(&engine), app_support.join("state.json"));
    match registry.load() {
        Ok(count) => eprintln!("dirijord-rs: loaded {count} session record(s)"),
        Err(error) => eprintln!("dirijord-rs: state load: {error}"),
    }
    let adopted = registry.restore(&holder, &logs_dir);
    eprintln!(
        "dirijord-rs: adopted {} live holder session(s): {adopted:?}",
        adopted.len()
    );
    let registry = Arc::new(Mutex::new(registry));

    let cli_path = exe_dir.join("dirijor");
    let server = Arc::new(
        ControlServer::new(Arc::clone(&registry), app_support.join("daemon.sock"))
            .with_logs_dir(&logs_dir)
            .with_holder(holder)
            .with_injection(InjectionConfig {
                inject_dir: app_support.join("inject"),
                cli_path,
            }),
    );
    let listener = match server.bind() {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("dirijord-rs: bind: {error}");
            // A live socket means a daemon is already serving; that is the
            // singleton working, not a failure.
            std::process::exit(if error.kind() == std::io::ErrorKind::AddrInUse {
                0
            } else {
                1
            });
        }
    };

    let _watcher = diri_engine::events::spawn_registry_watcher(
        Arc::clone(&registry),
        server.events(),
        Arc::new(AtomicBool::new(false)),
    );
    let _governor = diri_engine::governor::spawn_governor(
        Arc::clone(&registry),
        server.events(),
        server.attach_hub(),
        server.governor_config(),
        Arc::new(AtomicBool::new(false)),
    );
    let _pr_monitor = diri_engine::pr_monitor::spawn_pr_monitor(
        Arc::clone(&registry),
        server.events(),
        server.attach_hub(),
        Arc::new(AtomicBool::new(false)),
    );
    let _persist_flusher = diri_engine::registry::spawn_persist_flusher(
        Arc::clone(&registry),
        Arc::new(AtomicBool::new(false)),
    );

    eprintln!(
        "dirijord-rs: serving {}",
        server.socket_path().display()
    );
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let server = Arc::clone(&server);
                let _ = std::thread::Builder::new()
                    .name("dirijord-connection".into())
                    .spawn(move || {
                        let _ = server.serve(stream);
                    });
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => {
                eprintln!("dirijord-rs: accept: {error}");
                break;
            }
        }
    }
}

#[cfg(unix)]
fn login_path() -> Option<String> {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
    let output = std::process::Command::new(&shell)
        .args(["-l", "-c", "echo __DIRI_PATH__=$PATH"])
        .output()
        .ok()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let path = stdout
        .lines()
        .find_map(|line| line.strip_prefix("__DIRI_PATH__="))?
        .trim()
        .to_string();
    (!path.is_empty()).then_some(path)
}

#[cfg(unix)]
fn app_support_dir() -> PathBuf {
    if let Ok(root) = std::env::var("DIRIJOR_APP_SUPPORT") {
        return PathBuf::from(root);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    Path::new(&home).join("Library/Application Support/Dirijor")
}

#[cfg(unix)]
/// Base catalog next to the executable (the SPM resource bundle layout the
/// app ships), then user overrides, then dev-checkout fallbacks.
fn load_manifests(exe_dir: &Path, app_support: &Path) -> (ManifestEngine, Vec<String>) {
    let mut bases: Vec<PathBuf> = Vec::new();
    if let Ok(configured) = std::env::var("DIRI_MANIFESTS_DIR") {
        bases.push(PathBuf::from(configured));
    }
    bases.push(exe_dir.join("dirijor_DirijorCore.bundle/manifests"));
    bases.push(exe_dir.join("manifests"));
    // Dev checkout: crates/diri-engine target dirs sit under diri/.
    bases.push(exe_dir.join("../../../../Sources/DirijorCore/Resources/manifests"));
    let base = bases.into_iter().find(|dir| dir.is_dir());
    let overrides = app_support.join("manifests/overrides");

    let mut dirs: Vec<&Path> = Vec::new();
    if let Some(base) = &base {
        dirs.push(base);
    }
    dirs.push(&overrides);
    ManifestEngine::load_dirs(&dirs).unwrap_or_else(|error| {
        eprintln!("dirijord-rs: manifest load: {error}");
        (ManifestEngine::new(Vec::new()), Vec::new())
    })
}

#[cfg(unix)]
fn holder_executable(exe_dir: &Path) -> PathBuf {
    for name in ["diri-holder", "dirijord-holder"] {
        let candidate = exe_dir.join(name);
        if candidate.is_file() {
            return candidate;
        }
    }
    exe_dir.join("diri-holder")
}
