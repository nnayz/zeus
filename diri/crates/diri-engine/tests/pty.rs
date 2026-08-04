//! PTY behavior, exercised against real child processes.
//!
//! These spawn short-lived children of the test process only. They never touch
//! a daemon, a holder, or anything under the user's Application Support.

#![cfg(unix)]

use std::io::{Read, Write};
use std::time::{Duration, Instant};

use diri_engine::{Exit, Pty, PtySpec};

fn read_until(stream: &mut impl Read, needle: &str, within: Duration) -> String {
    let deadline = Instant::now() + within;
    let mut seen = String::new();
    let mut buffer = [0u8; 4096];
    while Instant::now() < deadline {
        match stream.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => {
                seen.push_str(&String::from_utf8_lossy(&buffer[..n]));
                if seen.contains(needle) {
                    return seen;
                }
            }
            Err(_) => break,
        }
    }
    seen
}

#[test]
fn a_child_runs_and_its_output_arrives_on_the_master() {
    let spec = PtySpec::new(
        vec![
            "/bin/sh".into(),
            "-c".into(),
            "printf 'hello-from-pty\\n'".into(),
        ],
        "/tmp",
    )
    .env("PATH", "/usr/bin:/bin");

    let mut pty = Pty::spawn(&spec).expect("spawn");
    let mut reader = pty.reader().expect("reader");
    let seen = read_until(&mut reader, "hello-from-pty", Duration::from_secs(10));
    assert!(seen.contains("hello-from-pty"), "got: {seen:?}");
    assert_eq!(pty.wait().expect("wait"), Exit::Code(0));
}

#[test]
fn the_child_gets_only_the_environment_it_was_given() {
    // A daemon's own environment must not leak: an inherited NO_COLOR is what
    // silently monochromed agent output in the Swift daemon.
    unsafe { std::env::set_var("NO_COLOR", "1") };

    let spec = PtySpec::new(
        vec![
            "/bin/sh".into(),
            "-c".into(),
            "printf 'no_color=[%s]\\n' \"$NO_COLOR\"".into(),
        ],
        "/tmp",
    )
    .env("PATH", "/usr/bin:/bin");

    let mut pty = Pty::spawn(&spec).expect("spawn");
    let mut reader = pty.reader().expect("reader");
    let seen = read_until(&mut reader, "no_color=", Duration::from_secs(10));
    let _ = pty.wait();

    unsafe { std::env::remove_var("NO_COLOR") };
    assert!(seen.contains("no_color=[]"), "NO_COLOR leaked in: {seen:?}");
}

#[test]
fn the_kernel_reports_the_size_the_child_was_given() {
    let spec = PtySpec::new(vec!["/bin/cat".into()], "/tmp")
        .env("PATH", "/usr/bin:/bin")
        .size(120, 40);

    let mut pty = Pty::spawn(&spec).expect("spawn");
    assert_eq!(pty.size().expect("size"), (120, 40));

    pty.resize(100, 30).expect("resize");
    assert_eq!(
        pty.size().expect("size"),
        (100, 30),
        "resize is read back from the kernel, not from what we asked for"
    );

    pty.terminate(Duration::from_secs(2)).expect("terminate");
}

#[test]
fn a_child_sees_sigwinch_after_a_resize() {
    // The regression this guards: a daemon that ignores SIGWINCH passes that
    // disposition through exec, and the agent never learns it was resized, so
    // it never repaints. The child must receive the signal.
    let script = "trap 'printf caught-winch\\\\n' WINCH; printf ready\\\\n; \
                  for _ in 1 2 3 4 5 6 7 8 9 10; do sleep 0.5; done";
    let spec = PtySpec::new(vec!["/bin/sh".into(), "-c".into(), script.into()], "/tmp")
        .env("PATH", "/usr/bin:/bin");

    let mut pty = Pty::spawn(&spec).expect("spawn");
    let mut reader = pty.reader().expect("reader");
    let ready = read_until(&mut reader, "ready", Duration::from_secs(10));
    assert!(ready.contains("ready"), "child never started: {ready:?}");

    pty.resize(90, 30).expect("resize");
    let seen = read_until(&mut reader, "caught-winch", Duration::from_secs(10));
    pty.terminate(Duration::from_secs(2)).expect("terminate");

    assert!(
        seen.contains("caught-winch"),
        "child did not receive SIGWINCH: {seen:?}"
    );
}

#[test]
fn input_written_to_the_master_reaches_the_child() {
    let spec = PtySpec::new(vec!["/bin/cat".into()], "/tmp").env("PATH", "/usr/bin:/bin");

    let mut pty = Pty::spawn(&spec).expect("spawn");
    let mut writer = pty.writer().expect("writer");
    let mut reader = pty.reader().expect("reader");

    writer.write_all(b"round-trip\n").expect("write");
    writer.flush().expect("flush");

    let seen = read_until(&mut reader, "round-trip", Duration::from_secs(10));
    pty.terminate(Duration::from_secs(2)).expect("terminate");
    assert!(seen.contains("round-trip"), "got: {seen:?}");
}

#[test]
fn terminate_kills_backgrounded_grandchildren_too() {
    // `setsid` in the child plus a group signal is what makes this work: a
    // backgrounded grandchild would otherwise outlive the session and be
    // reparented to init.
    let script = "sleep 300 & echo grandchild=$!; sleep 300";
    let spec = PtySpec::new(vec!["/bin/sh".into(), "-c".into(), script.into()], "/tmp")
        .env("PATH", "/usr/bin:/bin");

    let mut pty = Pty::spawn(&spec).expect("spawn");
    let mut reader = pty.reader().expect("reader");
    let seen = read_until(&mut reader, "grandchild=", Duration::from_secs(10));
    let grandchild: i32 = seen
        .split("grandchild=")
        .nth(1)
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|digits| digits.trim().parse().ok())
        .unwrap_or_else(|| panic!("no grandchild pid in {seen:?}"));

    pty.terminate(Duration::from_secs(3)).expect("terminate");

    // Give the kernel a moment to finish tearing the group down.
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut alive = true;
    while Instant::now() < deadline {
        // SAFETY: signal 0 only probes for existence.
        if unsafe { libc::kill(grandchild, 0) } != 0 {
            alive = false;
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(!alive, "grandchild {grandchild} outlived the session");
}

#[test]
fn a_child_killed_by_a_signal_reports_the_signal_not_an_exit_code() {
    let spec = PtySpec::new(vec!["/bin/cat".into()], "/tmp").env("PATH", "/usr/bin:/bin");
    let mut pty = Pty::spawn(&spec).expect("spawn");

    pty.kill_group(libc::SIGKILL).expect("kill");
    assert_eq!(pty.wait().expect("wait"), Exit::Signal(libc::SIGKILL));
}
