//! The same assertions run against direct/local Holder and fake-SSH Holder
//! Sessions. Only fixture construction differs; no real SSH host is used.

use std::sync::Barrier;
use std::time::{Duration, Instant};
use zeus_engine::Session;
use zeus_proto::terminal::*;
use zeus_proto::{ClientRole, SessionId};

pub const SCRIPT: &str = "stty -echo -icanon min 1 time 0; printf 'ready>'; exec cat";

pub struct Cleanup(pub Session);
impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = self.0.terminate(Duration::from_millis(200));
    }
}

pub fn owner(id: &str, role: ClientRole) -> Controller {
    Controller {
        id: id.into(),
        label: id.into(),
        role,
    }
}

pub fn snapshot(session: &Session, since: Option<SnapshotCursor>) -> TerminalSnapshot {
    session
        .terminal_snapshot(&TerminalSnapshotParams {
            session_id: SessionId(session.id().into()),
            protocol: TERMINAL_PROTOCOL,
            since,
        })
        .unwrap()
}

fn text(snapshot: &TerminalSnapshot) -> String {
    snapshot
        .decode_grid()
        .unwrap()
        .unwrap()
        .changed_rows
        .iter()
        .flat_map(|r| &r.cells)
        .map(|c| char::from_u32(c.scalar).unwrap_or(' '))
        .collect()
}

pub fn wait_for(session: &Session, marker: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(s) = session.terminal_snapshot(&TerminalSnapshotParams {
            session_id: SessionId(session.id().into()),
            protocol: 1,
            since: None,
        }) && text(&s).contains(marker)
        {
            return;
        }
        assert!(Instant::now() < deadline, "waiting for {marker}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

pub fn assert_contract(session: &Session) {
    wait_for(session, "ready>");
    let initial = snapshot(session, None);
    assert!(initial.control.owner.is_none());
    let grid = initial.decode_grid().unwrap().unwrap();
    assert_eq!((grid.cols, grid.rows), (80, 24));
    assert_eq!(session.screen_size(), (80, 24));
    let desktop = session
        .acquire_terminal_control(
            &initial.control.epoch,
            owner("desktop", ClientRole::Desktop),
            false,
        )
        .unwrap();
    assert_eq!(
        session
            .acquire_terminal_control(&desktop.epoch, owner("phone", ClientRole::Mobile), false)
            .unwrap_err()
            .code,
        "controller_busy"
    );
    let mobile = session
        .acquire_terminal_control(&desktop.epoch, owner("phone", ClientRole::Mobile), true)
        .unwrap();
    assert_eq!(
        session
            .validate_terminal_control(&desktop.epoch, "desktop")
            .unwrap_err()
            .code,
        "stale_controller_epoch"
    );
    assert_eq!(
        session
            .release_terminal_control(&desktop.epoch, "desktop")
            .unwrap_err()
            .code,
        "stale_controller_epoch"
    );
    assert_eq!(
        session
            .validate_terminal_control(&mobile.epoch, "intruder")
            .unwrap_err()
            .code,
        "not_controller"
    );
    let command = TerminalSendTextParams {
        session_id: SessionId(session.id().into()),
        expected: mobile.epoch.clone(),
        owner_id: "phone".into(),
        command_seq: 1,
        text: "mobile-marker".into(),
        submit: false,
    };
    let consumed = session.terminal_send_text(&command).unwrap();
    assert_eq!(consumed.command_seq, 1);
    assert_eq!(
        session.terminal_send_text(&command).unwrap_err().code,
        "command_sequence"
    );
    wait_for(session, "mobile-marker");
    let mut malformed = command.clone();
    malformed.command_seq = 2;
    malformed.text = "\x1b[2J".into();
    assert_eq!(
        session.terminal_send_text(&malformed).unwrap_err().code,
        "bad_request"
    );
    malformed.text = "x".repeat(MAX_TEXT_BYTES + 1);
    assert_eq!(
        session.terminal_send_text(&malformed).unwrap_err().code,
        "bad_request"
    );
    assert_eq!(session.terminal_control_state().command_seq, 1);

    // Reconnect is observation only: same lease/process, no mutation replay.
    let restored = snapshot(session, Some(initial.cursor));
    assert!(restored.grid.is_some());
    assert_eq!(restored.control, consumed);
    let current = snapshot(session, Some(restored.cursor.clone()));
    assert!(current.grid.is_none());
    assert_eq!(current.control, consumed);
    let foreign = SnapshotCursor {
        incarnation: "old-engine".into(),
        sequence: restored.cursor.sequence,
    };
    assert!(snapshot(session, Some(foreign)).grid.is_some());
    let future = SnapshotCursor {
        sequence: u64::MAX,
        ..restored.cursor.clone()
    };
    assert!(snapshot(session, Some(future)).grid.is_some());
    assert_eq!(session.screen_size(), (80, 24), "viewing never resizes");

    // All modes survive full reseed, even when no visible cell changes.
    session.write_input(b"\x1b[?1049h\x1b[?1h\x1b[?2004h\x1b[?1003h\x1b[?1005h\x1b[?1004h\x1b[?1007lmode-marker").unwrap();
    wait_for(session, "mode-marker");
    let modes = snapshot(session, Some(restored.cursor));
    assert!(modes.grid.is_some());
    assert!(
        modes.modes.alt_screen
            && modes.modes.application_cursor_keys
            && modes.modes.bracketed_paste
    );
    assert!(
        modes.modes.mouse_reporting
            && modes.modes.mouse_motion
            && modes.modes.mouse_utf8
            && modes.modes.focus_reporting
    );
    assert!(!modes.modes.alternate_scroll);

    // An absent/slow viewer accumulates no replay. A later request simply
    // replaces its old full snapshot with the authoritative current state.
    let flood = format!("{}latest-marker", "busy-output\r\n".repeat(4000));
    session.write_input(flood.as_bytes()).unwrap();
    wait_for(session, "latest-marker");
    let recovered = snapshot(session, Some(modes.cursor));
    assert!(text(&recovered).contains("latest-marker"));
    assert_eq!(recovered.control, consumed);
    let mut timings = Vec::new();
    for _ in 0..30 {
        let before = Instant::now();
        let value = snapshot(session, None);
        assert!(value.grid.as_ref().unwrap().len() <= MAX_SNAPSHOT_BYTES);
        timings.push(before.elapsed());
    }
    timings.sort_unstable();
    eprintln!(
        "Companion Engine snapshot p90: {}us",
        timings[27].as_micros()
    );
    assert!(timings[27] <= Duration::from_millis(100));

    let barrier = Barrier::new(4);
    let expected = session.terminal_control_state().epoch;
    let winners = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..4)
            .map(|i| {
                let barrier = &barrier;
                let expected = &expected;
                scope.spawn(move || {
                    barrier.wait();
                    session.acquire_terminal_control(
                        expected,
                        owner(&format!("racer-{i}"), ClientRole::Mobile),
                        true,
                    )
                })
            })
            .collect();
        handles
            .into_iter()
            .filter_map(|h| h.join().unwrap().ok())
            .collect::<Vec<_>>()
    });
    assert_eq!(winners.len(), 1);
    assert_eq!(
        session.terminal_send_text(&command).unwrap_err().code,
        "stale_controller_epoch"
    );
    let winner = &winners[0];
    let released = session
        .release_terminal_control(&winner.epoch, &winner.owner.as_ref().unwrap().id)
        .unwrap();
    assert!(released.owner.is_none());
    assert!(released.epoch.generation > winner.epoch.generation);
    assert_eq!(
        session
            .validate_terminal_control(&winner.epoch, &winner.owner.as_ref().unwrap().id)
            .unwrap_err()
            .code,
        "stale_controller_epoch"
    );
}
