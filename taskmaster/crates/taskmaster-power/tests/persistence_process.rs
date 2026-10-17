#![cfg(unix)]

use std::fs;
use std::io::{BufRead, BufReader};
use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Stdio};
use std::sync::Mutex;

use taskmaster_power::persistence::{LockedStateDirectory, PersistenceError};

const CHILD_ENV: &str = "TASKMASTER_POWER_LOCK_CHILD";

/// `flock` releases only when every descriptor of the open file description is
/// closed, and a forked child keeps a pre-`exec` copy of every descriptor
/// alive. Serialize these process-lock tests so one test's fork cannot delay
/// another test's release and re-acquisition.
static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> std::sync::MutexGuard<'static, ()> {
    SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn uid() -> u32 {
    rustix::process::getuid().as_raw()
}

fn fixture() -> tempfile::TempDir {
    let directory = tempfile::tempdir().unwrap();
    fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700)).unwrap();
    directory
}

#[test]
fn child_lock_probe() {
    let Ok(path) = std::env::var(CHILD_ENV) else {
        return;
    };
    let result = LockedStateDirectory::acquire(path.as_ref(), uid(), [9; 16]);
    assert!(matches!(result, Err(PersistenceError::AlreadyLocked)));
}

#[test]
fn a_second_process_cannot_enter_recovery_while_the_helper_is_live() {
    let _serial = serial();
    let directory = fixture();
    let first = LockedStateDirectory::acquire(directory.path(), uid(), [3; 16]).unwrap();
    let status = Command::new(std::env::current_exe().unwrap())
        .arg("--exact")
        .arg("child_lock_probe")
        .arg("--nocapture")
        .env(CHILD_ENV, directory.path())
        .status()
        .unwrap();
    assert!(status.success());
    drop(first);
    LockedStateDirectory::acquire(directory.path(), uid(), [4; 16])
        .expect("the successor enters only after the first lifetime ends");
}

#[test]
fn lifetime_lock_is_closed_across_exec() {
    let _serial = serial();
    let directory = fixture();
    let first = LockedStateDirectory::acquire(directory.path(), uid(), [3; 16]).unwrap();
    // The successor check must start only after the child has reached `exec`:
    // a pre-`exec` copy of the inherited descriptor keeps the flock alive past
    // `drop`, and a `CLOEXEC` descriptor is closed exactly at `exec`.
    let mut unrelated_child = Command::new("/bin/sh")
        .args(["-c", "echo ready; exec sleep 5"])
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut ready = String::new();
    BufReader::new(unrelated_child.stdout.take().unwrap())
        .read_line(&mut ready)
        .unwrap();
    assert_eq!(ready.trim(), "ready");
    drop(first);
    LockedStateDirectory::acquire(directory.path(), uid(), [4; 16])
        .expect("an exec child must not inherit the Helper lifetime lock");
    let _ = unrelated_child.kill();
    unrelated_child.wait().unwrap();
}
