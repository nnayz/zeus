//! The binary data channel over a real socket: attach, get seeded, type,
//! see grid diffs — the app's terminal path against the Rust engine.

#![cfg(unix)]

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::json;
use zeus_engine::control::ControlServer;
use zeus_engine::detect::ManifestEngine;
use zeus_engine::registry::Registry;
use zeus_proto::ControlMessage;
use zeus_proto::frames::{Frame, FrameCodec, FrameType, TerminalModes};
use zeus_proto::grid::GridUpdate;

fn engine() -> Arc<ManifestEngine> {
    let dir = zeus_engine::detect::bundled_manifest_dir()
        .canonicalize()
        .expect("manifests");
    let (engine, _) = ManifestEngine::load_dir(&dir).expect("load");
    Arc::new(engine)
}

/// Decoded-frame reader that never drops frames arriving in one batch.
struct FrameReader {
    stream: UnixStream,
    codec: FrameCodec,
    queue: std::collections::VecDeque<Frame>,
}

impl FrameReader {
    fn new(stream: UnixStream) -> Self {
        Self {
            stream,
            codec: FrameCodec::new(),
            queue: std::collections::VecDeque::new(),
        }
    }

    /// Pops frames (reading more as needed) until `predicate` matches.
    fn until(&mut self, what: &str, mut predicate: impl FnMut(&Frame) -> bool) -> Frame {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut chunk = [0u8; 64 << 10];
        loop {
            if let Some(frame) = self.queue.pop_front() {
                if predicate(&frame) {
                    return frame;
                }
                continue;
            }
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .unwrap_or_else(|| panic!("timed out waiting for {what}"));
            self.stream
                .set_read_timeout(Some(remaining))
                .expect("set frame read timeout");
            let count = self.stream.read(&mut chunk).unwrap_or_else(|error| {
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) {
                    panic!("timed out waiting for {what}");
                }
                panic!("read frames while waiting for {what}: {error}");
            });
            assert!(count > 0, "data channel closed while waiting for {what}");
            self.queue
                .extend(self.codec.feed(&chunk[..count]).expect("valid frames"));
        }
    }
}

fn grid_text(update: &GridUpdate) -> String {
    update
        .changed_rows
        .iter()
        .map(|row| {
            row.cells
                .iter()
                .map(|cell| char::from_u32(cell.scalar).unwrap_or(' '))
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn an_attach_is_seeded_then_streams_diffs_and_answers_input() {
    let temp = tempfile::tempdir().expect("temp");
    let registry = Arc::new(Mutex::new(Registry::new(
        engine(),
        temp.path().join("state.json"),
    )));
    let server = Arc::new(
        ControlServer::new(Arc::clone(&registry), temp.path().join("daemon.sock"))
            .with_logs_dir(temp.path().join("logs")),
    );
    let listener = server.bind().expect("bind");
    {
        let server = Arc::clone(&server);
        std::thread::spawn(move || {
            while let Ok((stream, _)) = listener.accept() {
                let server = Arc::clone(&server);
                std::thread::spawn(move || {
                    let _ = server.serve(stream);
                });
            }
        });
    }

    // Control connection: spawn a cat session that echoes what we type.
    let control = UnixStream::connect(server.socket_path()).expect("connect control");
    let send = |message: &ControlMessage| {
        let mut bytes = serde_json::to_vec(message).expect("encode");
        bytes.push(b'\n');
        (&control).write_all(&bytes).expect("write");
    };
    send(&ControlMessage::Request {
        id: 1,
        method: "session.spawn".into(),
        params: Some(json!({
            "kind": { "shell": {} },
            "cwd": "/tmp",
            "argv": ["/bin/sh", "-c", "printf 'seeded-screen\\n'; stty -echo -icanon min 1 time 0; exec cat"],
        })),
    });
    let mut reader = std::io::BufReader::new(control.try_clone().expect("clone"));
    let id = {
        use std::io::BufRead;
        let mut line = String::new();
        reader.read_line(&mut line).expect("spawn reply");
        let reply: ControlMessage = serde_json::from_str(&line).expect("decode");
        match reply {
            ControlMessage::Response {
                result: Ok(result), ..
            } => result["id"].as_str().expect("id").to_string(),
            other => panic!("spawn failed: {other:?}"),
        }
    };

    // Give the child a beat to print its banner so the seed contains it.
    std::thread::sleep(Duration::from_millis(400));

    // Data channel: one JSON line, then binary frames.
    let mut data = UnixStream::connect(server.socket_path()).expect("connect data");
    let mut attach_line = serde_json::to_vec(&json!({ "attach": id })).expect("encode");
    attach_line.push(b'\n');
    data.write_all(&attach_line).expect("attach");

    let mut frames = FrameReader::new(data.try_clone().expect("clone data"));
    let seed = frames.until("the seed grid", |frame| frame.frame_type == FrameType::Grid);
    let update = seed.grid_payload().expect("decode").expect("grid");
    assert!(
        update.is_full_snapshot,
        "a fresh sink gets the whole screen"
    );
    assert!(
        grid_text(&update).contains("seeded-screen"),
        "the seed carries what the child already painted"
    );

    let modes = frames.until("initial modes", |frame| {
        frame.frame_type == FrameType::Modes
    });
    assert_eq!(
        modes.modes_payload(),
        Some(TerminalModes {
            alternate_scroll: true,
            ..TerminalModes::default()
        })
    );

    // Let the per-session pump establish its shared diff baseline. Its first
    // sample is allowed to be a FullSnapshot: if input beats that first tick,
    // the snapshot legitimately includes the new text. A second turn is the
    // deterministic seam for asserting steady-state diff behavior.
    data.write_all(&FrameCodec::encode(&Frame::input(b"warm-up-pump\n".to_vec())).expect("encode"))
        .expect("send warm-up input");
    frames.until("the warm-up echo", |frame| {
        frame.frame_type == FrameType::Grid
            && frame
                .grid_payload()
                .ok()
                .flatten()
                .is_some_and(|update| grid_text(&update).contains("warm-up-pump"))
    });

    // Typing through the established data channel: cat echoes, and each echo
    // comes back as a grid DIFF (not a full snapshot). Use the median so a
    // single scheduler hiccup cannot fail the test, while a fixed 16 ms frame
    // boundary on every keystroke still does.
    let mut interactive_latencies = Vec::new();
    for index in 0..5 {
        let marker = format!("typed-over-attach-{index}");
        let sent_at = Instant::now();
        data.write_all(
            &FrameCodec::encode(&Frame::input(format!("{marker}\n").into_bytes())).expect("encode"),
        )
        .expect("send input");
        let diff = frames.until("the echo diff", |frame| {
            frame.frame_type == FrameType::Grid
                && frame
                    .grid_payload()
                    .ok()
                    .flatten()
                    .is_some_and(|update| grid_text(&update).contains(&marker))
        });
        interactive_latencies.push(sent_at.elapsed());
        let update = diff.grid_payload().expect("decode").expect("grid");
        assert!(
            !update.is_full_snapshot,
            "steady-state frames are diffs, not full repaints"
        );
    }
    interactive_latencies.sort_unstable();
    let median = interactive_latencies[interactive_latencies.len() / 2];
    eprintln!("local input-to-grid median: {}us", median.as_micros());
    assert!(
        median <= Duration::from_millis(8),
        "local input-to-grid median was {median:?}; expected no fixed 16 ms frame boundary"
    );

    // Ping answers pong on the same channel.
    data.write_all(&FrameCodec::encode(&Frame::ping()).expect("encode"))
        .expect("send ping");
    frames.until("pong", |frame| frame.frame_type == FrameType::Pong);

    // A resize through the data channel reshapes the PTY; the next grid
    // carries the new geometry.
    data.write_all(&FrameCodec::encode(&Frame::resize(100, 30)).expect("encode"))
        .expect("send resize");
    frames.until("resized grid", |frame| {
        frame.frame_type == FrameType::Grid
            && frame
                .grid_payload()
                .ok()
                .flatten()
                .is_some_and(|update| update.cols == 100 && update.rows == 30)
    });

    // Mode-only output must cross the same channel even when it changes no
    // visible cells. This is the state the pane uses for arrow and paste
    // encoding, so it cannot wait for or be inferred from a later grid.
    data.write_all(
        &FrameCodec::encode(&Frame::input(
            b"\x1b[?1h\x1b[?2004h\x1b[?1003h\x1b[?1005h\x1b[?1007l\x1b[?1004h".to_vec(),
        ))
        .expect("encode"),
    )
    .expect("enable input modes");
    let enabled = frames.until("enabled input modes", |frame| {
        frame.modes_payload().is_some_and(|modes| {
            modes.application_cursor_keys
                && modes.bracketed_paste
                && modes.mouse_reporting
                && modes.mouse_utf8
                && modes.mouse_motion
                && !modes.alternate_scroll
                && modes.focus_reporting
        })
    });
    assert_eq!(
        enabled.modes_payload(),
        Some(TerminalModes {
            application_cursor_keys: true,
            bracketed_paste: true,
            mouse_reporting: true,
            mouse_utf8: true,
            mouse_motion: true,
            focus_reporting: true,
            ..TerminalModes::default()
        })
    );

    // A fresh attachment is seeded from the authoritative screen modes, not
    // from client defaults or only from changes observed after reconnect.
    drop(frames);
    drop(data);
    let mut data = UnixStream::connect(server.socket_path()).expect("reconnect data");
    let mut attach_line = serde_json::to_vec(&json!({ "attach": id })).expect("encode");
    attach_line.push(b'\n');
    data.write_all(&attach_line).expect("reattach");
    let mut frames = FrameReader::new(data.try_clone().expect("clone reattached data"));
    frames.until("reattached seed grid", |frame| {
        frame.frame_type == FrameType::Grid
    });
    let reattached = frames.until("reattached input modes", |frame| {
        frame.frame_type == FrameType::Modes
    });
    assert_eq!(reattached.modes_payload(), enabled.modes_payload());

    data.write_all(
        &FrameCodec::encode(&Frame::input(
            b"\x1b[?1l\x1b[?2004l\x1b[?1003l\x1b[?1005l\x1b[?1004l".to_vec(),
        ))
        .expect("encode"),
    )
    .expect("disable input modes");
    let disabled = frames.until("disabled input modes", |frame| {
        frame.modes_payload().is_some_and(|modes| {
            !modes.application_cursor_keys
                && !modes.bracketed_paste
                && !modes.mouse_reporting
                && !modes.mouse_utf8
                && !modes.mouse_motion
                && !modes.alternate_scroll
                && !modes.focus_reporting
        })
    });
    assert_eq!(disabled.modes_payload(), Some(TerminalModes::default()));

    // Clean up the child.
    send(&ControlMessage::Request {
        id: 2,
        method: "session.kill".into(),
        params: Some(json!({ "sessionID": id })),
    });
}
