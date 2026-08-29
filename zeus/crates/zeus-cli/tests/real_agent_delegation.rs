//! Opt-in acceptance test for hosted-agent orchestration.
//!
//! This intentionally talks to a running development Zeus daemon and consumes
//! a real provider turn. Normal test runs ignore it.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;
use zeus_proto::SessionRecord;

const OPT_IN_ENV: &str = "ZEUS_REAL_AGENT_SMOKE";

fn cli() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_zeus-cli"))
}

fn run(args: &[&str]) -> Output {
    Command::new(cli())
        .args(args)
        .output()
        .unwrap_or_else(|error| panic!("could not launch zeus-cli {args:?}: {error}"))
}

fn run_ok(args: &[&str]) -> Output {
    let output = run(args);
    assert!(
        output.status.success(),
        "zeus-cli {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn sessions() -> Vec<SessionRecord> {
    let output = run_ok(&["session", "list", "--all", "--json"]);
    let envelope: Value = serde_json::from_slice(&output.stdout).expect("session list JSON");
    serde_json::from_value(envelope["sessions"].clone()).expect("SessionRecord list")
}

struct CreatedSessions(Vec<String>);

impl CreatedSessions {
    fn track(&mut self, id: impl Into<String>) {
        let id = id.into();
        if !self.0.contains(&id) {
            self.0.push(id);
        }
    }
}

impl Drop for CreatedSessions {
    fn drop(&mut self) {
        for id in self.0.iter().rev() {
            let _ = run(&["session", "release", id, "--remove", "--json"]);
        }
    }
}

#[test]
#[ignore = "set ZEUS_REAL_AGENT_SMOKE=1 and run against the current development daemon"]
fn natural_language_delegation_creates_a_child_session_record() {
    assert_eq!(
        std::env::var(OPT_IN_ENV).as_deref(),
        Ok("1"),
        "this test creates real Zeus sessions and consumes a provider turn"
    );

    let kind = std::env::var("ZEUS_REAL_AGENT_KIND").unwrap_or_else(|_| "codex".into());
    let cwd = std::env::var_os("ZEUS_REAL_AGENT_CWD")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().expect("current directory"));
    assert!(
        cwd.is_dir(),
        "smoke-test cwd does not exist: {}",
        cwd.display()
    );
    let timeout = std::env::var("ZEUS_REAL_AGENT_TIMEOUT_SECONDS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .map(Duration::from_secs)
        .unwrap_or_else(|| Duration::from_secs(300));

    // The prompt deliberately does not mention Zeus, MCP, a tool name, or a
    // provider-specific spawning feature. That is the behavior under test.
    let cwd = path_arg(&cwd);
    let spawned = run_ok(&[
        "session",
        "spawn",
        &kind,
        "--cwd",
        &cwd,
        "--title",
        "issue-48 delegation smoke",
        "--prompt",
        "Spawn a subagent to review this implementation and report back. Do not make code changes.",
        "--json",
    ]);
    let parent: SessionRecord =
        serde_json::from_slice(&spawned.stdout).expect("spawned parent SessionRecord");
    let mut cleanup = CreatedSessions(vec![parent.id.0.clone()]);

    let deadline = Instant::now() + timeout;
    loop {
        let records = sessions();
        let children: Vec<&SessionRecord> = records
            .iter()
            .filter(|record| record.parent.as_ref() == Some(&parent.id))
            .collect();
        if !children.is_empty() {
            for child in children {
                cleanup.track(child.id.0.clone());
                assert_eq!(
                    child.parent.as_ref(),
                    Some(&parent.id),
                    "child SessionRecord lost its parent lineage"
                );
            }
            return;
        }

        assert!(
            Instant::now() < deadline,
            "{kind} did not create a Zeus child session within {} seconds; parent={} output:\n{}",
            timeout.as_secs(),
            parent.id.0,
            String::from_utf8_lossy(
                &run(&["session", "read", &parent.id.0, "--lines", "80", "--json"]).stdout
            )
        );
        thread::sleep(Duration::from_secs(2));
    }
}

fn path_arg(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}
