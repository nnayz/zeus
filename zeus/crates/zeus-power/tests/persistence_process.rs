#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::Command;

use zeus_power::persistence::{LockedStateDirectory, PersistenceError};

const CHILD_ENV: &str = "ZEUS_POWER_LOCK_CHILD";

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
    let directory = fixture();
    let first = LockedStateDirectory::acquire(directory.path(), uid(), [3; 16]).unwrap();
    let mut unrelated_child = Command::new("/bin/sleep").arg("1").spawn().unwrap();
    drop(first);
    LockedStateDirectory::acquire(directory.path(), uid(), [4; 16])
        .expect("an exec child must not inherit the Helper lifetime lock");
    unrelated_child.kill().unwrap();
    unrelated_child.wait().unwrap();
}
