#![cfg(unix)]

#[path = "support/terminal_contract.rs"]
mod contract;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use zeus_engine::{Authority, HolderConfig, ManifestEngine, PtySpec, Session, SessionSpec};

#[test]
fn companion_contract_matches_direct_and_local_holder_sessions() {
    for held in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let holder = held.then(|| HolderConfig {
            holders_dir: temp.path().join("holders"),
            executable: PathBuf::from(env!("CARGO_BIN_EXE_zeus-holder")),
        });
        let session = Session::spawn(
            SessionSpec {
                id: "terminal-contract".into(),
                pty: PtySpec::new(
                    vec!["/bin/sh".into(), "-c".into(), contract::SCRIPT.into()],
                    "/tmp",
                )
                .env("PATH", "/usr/bin:/bin")
                .env("TERM", "xterm-256color")
                .size(80, 24),
                manifest_id: "shell".into(),
                authority: Authority::ProcessOnly,
                logs_dir: temp.path().join("logs"),
                holder,
                remote: None,
                defer_launch: false,
            },
            Arc::new(ManifestEngine::new(Vec::new())),
        )
        .unwrap();
        let mut session = contract::Cleanup(session);
        contract::assert_contract(&session.0);
        session.0.terminate(Duration::from_millis(200)).unwrap();
        assert!(contract::snapshot(&session.0, None).exited);
    }
}
