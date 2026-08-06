//! zeusd-rs — the Rust Zeus daemon.
//!
//! Drop-in for the Swift `zeusd`: same socket, same on-disk state, same
//! wire protocol, same holder adoption. Point the app at it with
//! `ZEUSD_PATH` (the launch override `daemon_launch.rs` already honors) to
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
    // Stamp process start on stderr: captured into zeusd.boot.log by the
    // app's launcher, and our only visibility for pre-log failures.
    eprintln!(
        "zeusd-rs: process start pid={} build=zeus-engine-{}",
        std::process::id(),
        env!("CARGO_PKG_VERSION")
    );

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
        eprintln!("zeusd-rs: {} manifest file(s) failed to parse: {failed:?}", failed.len());
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

    let cli_path = exe_dir.join("zeus");
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

    let _watcher = zeus_engine::events::spawn_registry_watcher(
        Arc::clone(&registry),
        server.events(),
        Arc::new(AtomicBool::new(false)),
    );

    eprintln!(
        "zeusd-rs: serving {}",
        server.socket_path().display()
    );
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

#[cfg(unix)]
fn app_support_dir() -> PathBuf {
    if let Ok(root) = std::env::var("ZEUS_APP_SUPPORT") {
        return PathBuf::from(root);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    Path::new(&home).join("Library/Application Support/Zeus")
}

#[cfg(unix)]
/// Base catalog next to the executable (the SPM resource bundle layout the
/// app ships), then user overrides, then dev-checkout fallbacks.
fn load_manifests(exe_dir: &Path, app_support: &Path) -> (ManifestEngine, Vec<String>) {
    let mut bases: Vec<PathBuf> = Vec::new();
    if let Ok(configured) = std::env::var("ZEUS_MANIFESTS_DIR") {
        bases.push(PathBuf::from(configured));
    }
    bases.push(exe_dir.join("zeus_ZeusCore.bundle/manifests"));
    bases.push(exe_dir.join("manifests"));
    // Dev checkout: crates/zeus-engine target dirs sit under zeus/.
    bases.push(exe_dir.join("../../../../Sources/ZeusCore/Resources/manifests"));
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
    for name in ["zeus-holder", "zeusd-holder"] {
        let candidate = exe_dir.join(name);
        if candidate.is_file() {
            return candidate;
        }
    }
    exe_dir.join("zeus-holder")
}
