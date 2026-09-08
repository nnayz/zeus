#![cfg(unix)]

use serde_json::{Value, json};
use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use zeus_engine::{ManifestEngine, Registry, control::ControlServer};
use zeus_proto::frames::{Frame, FrameCodec, FrameType};
use zeus_proto::terminal::*;
use zeus_proto::{ControlError, ControlMessage};

fn connection(server: &Arc<ControlServer>) -> UnixStream {
    let (client, engine) = UnixStream::pair().unwrap();
    let server = Arc::clone(server);
    std::thread::spawn(move || {
        let _ = server.serve(engine);
    });
    client
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    client
}

fn rpc(server: &Arc<ControlServer>, method: &str, params: Value) -> Result<Value, ControlError> {
    let mut socket = connection(server);
    let message = ControlMessage::Request {
        id: 1,
        method: method.into(),
        params: Some(params),
    };
    socket
        .write_all(&zeus_proto::control::encode_line(&message).unwrap())
        .unwrap();
    let mut line = Vec::new();
    BufReader::new(socket).read_until(b'\n', &mut line).unwrap();
    let ControlMessage::Response { result, .. } = zeus_proto::control::decode_line(&line).unwrap()
    else {
        panic!("response")
    };
    result
}

struct Desktop {
    socket: UnixStream,
    codec: FrameCodec,
    queue: VecDeque<Frame>,
}
impl Desktop {
    fn open(server: &Arc<ControlServer>, id: &str) -> (Self, AttachmentControlState) {
        let mut socket = connection(server);
        writeln!(
            socket,
            "{}",
            json!({"attach":id,"role":"desktop","controlProtocol":1})
        )
        .unwrap();
        let mut desktop = Self {
            socket,
            codec: FrameCodec::new(),
            queue: VecDeque::new(),
        };
        let state = desktop.control(|_| true);
        desktop.until(|f| f.frame_type == FrameType::Grid);
        (desktop, state)
    }
    fn until(&mut self, predicate: impl Fn(&Frame) -> bool) -> Frame {
        loop {
            if let Some(frame) = self.queue.pop_front() {
                if predicate(&frame) {
                    return frame;
                }
                continue;
            }
            let mut buf = [0; 65536];
            let n = self.socket.read(&mut buf).unwrap();
            assert!(n > 0, "attach closed");
            self.queue.extend(self.codec.feed(&buf[..n]).unwrap());
        }
    }
    fn control(
        &mut self,
        predicate: impl Fn(&AttachmentControlState) -> bool,
    ) -> AttachmentControlState {
        let frame = self.until(|f| {
            f.frame_type == FrameType::Controller
                && serde_json::from_slice::<AttachmentControlState>(&f.payload)
                    .is_ok_and(|c| predicate(&c))
        });
        serde_json::from_slice(&frame.payload).unwrap()
    }
    fn send(&mut self, epoch: &ControlEpoch, action: AttachmentAction) {
        let payload = serde_json::to_vec(&ControlledFrame {
            expected: epoch.clone(),
            action,
        })
        .unwrap();
        self.socket
            .write_all(&FrameCodec::encode(&Frame::new(FrameType::Controlled, payload)).unwrap())
            .unwrap();
    }
}

#[test]
fn desktop_mobile_handoff_rejects_stale_input_resize_and_unfenced_mutations() {
    let temp = tempfile::tempdir().unwrap();
    let registry = Arc::new(Mutex::new(Registry::new(
        Arc::new(
            ManifestEngine::load_dir(&zeus_engine::detect::bundled_manifest_dir())
                .unwrap()
                .0,
        ),
        temp.path().join("state.json"),
    )));
    let server = Arc::new(
        ControlServer::new(Arc::clone(&registry), temp.path().join("daemon.sock"))
            .with_logs_dir(temp.path().join("logs")),
    );
    let spawned = rpc(&server, "session.spawn", json!({"kind":{"shell":{}},"cwd":"/tmp","initialCols":80,"initialRows":24,"argv":["/bin/sh","-c","stty -echo -icanon min 1 time 0; printf 'ready>'; exec cat"]})).unwrap();
    let id = spawned["id"].as_str().unwrap();
    let (mut desktop, initial) = Desktop::open(&server, id);
    let mobile: ControlState = serde_json::from_value(rpc(&server, ACQUIRE_CONTROL, json!({"sessionID":id,"expected":initial.control.epoch,"owner":{"id":"phone","label":"Phone","role":"mobile"},"takeover":true})).unwrap()).unwrap();
    let revoked = desktop.control(|c| c.control.epoch == mobile.epoch);
    assert_eq!(revoked.control.owner.as_ref().unwrap().label, "Phone");

    desktop.send(
        &initial.control.epoch,
        AttachmentAction::Input {
            bytes: b"forbidden".to_vec(),
        },
    );
    assert_eq!(
        desktop.control(|c| c.error.is_some()).error.unwrap().code,
        "stale_controller_epoch"
    );
    desktop.send(
        &initial.control.epoch,
        AttachmentAction::Resize {
            cols: 120,
            rows: 40,
        },
    );
    assert_eq!(
        desktop.control(|c| c.error.is_some()).error.unwrap().code,
        "stale_controller_epoch"
    );
    for (method, extra) in [
        (
            "session.send_text",
            json!({"text":"forbidden","submit":false}),
        ),
        ("session.resize", json!({"cols":120,"rows":40})),
        ("session.kill", json!({})),
    ] {
        let mut params = extra;
        params["sessionID"] = json!(id);
        assert_eq!(
            rpc(&server, method, params).unwrap_err().code,
            "controller_busy"
        );
    }
    let snap: TerminalSnapshot = serde_json::from_value(
        rpc(&server, SNAPSHOT, json!({"sessionID":id,"protocol":1})).unwrap(),
    )
    .unwrap();
    assert_eq!(snap.decode_grid().unwrap().unwrap().cols, 80);
    assert!(
        !registry
            .lock()
            .unwrap()
            .get(id)
            .unwrap()
            .screen_lines()
            .join("\n")
            .contains("forbidden")
    );

    // Opening a new desktop cannot implicitly steal the phone's lease.
    let (_second, observed) = Desktop::open(&server, id);
    assert_eq!(observed.control.epoch, mobile.epoch);
    desktop.send(&mobile.epoch, AttachmentAction::TakeControl);
    let regained = desktop.control(|c| c.control.epoch.generation > mobile.epoch.generation);
    assert_eq!(
        regained.control.owner.as_ref().unwrap().id,
        initial.client_id
    );
    assert_eq!(rpc(&server, SEND_TEXT, json!({"sessionID":id,"expected":mobile.epoch,"ownerId":"phone","commandSeq":1,"text":"forbidden","submit":false})).unwrap_err().code, "stale_controller_epoch");
    desktop.send(
        &regained.control.epoch,
        AttachmentAction::Resize {
            cols: 100,
            rows: 30,
        },
    );
    desktop.until(|f| {
        f.grid_payload()
            .ok()
            .flatten()
            .is_some_and(|g| g.cols == 100 && g.rows == 30)
    });
    desktop.send(&regained.control.epoch, AttachmentAction::ReleaseControl);
    assert!(
        desktop
            .control(|c| c.control.owner.is_none())
            .control
            .owner
            .is_none()
    );
    rpc(&server, "session.kill", json!({"sessionID":id})).unwrap();
}

#[test]
fn mobile_cannot_open_binary_attachment_or_request_terminal_mutation_frames() {
    let temp = tempfile::tempdir().unwrap();
    let registry = Arc::new(Mutex::new(Registry::new(
        Arc::new(
            ManifestEngine::load_dir(&zeus_engine::detect::bundled_manifest_dir())
                .unwrap()
                .0,
        ),
        temp.path().join("state.json"),
    )));
    let server = Arc::new(ControlServer::new(
        registry,
        temp.path().join("daemon.sock"),
    ));
    let mut client = connection(&server);
    writeln!(
        client,
        "{}",
        json!({"attach":"anything","role":"mobile","controlProtocol":1})
    )
    .unwrap();
    assert_eq!(client.read(&mut [0u8; 1]).unwrap(), 0);
    for kind in ["signal", "terminate", "resize"] {
        assert!(rpc(&server, &format!("terminal.{kind}"), json!({})).is_err());
    }
}

#[test]
fn slow_snapshot_socket_does_not_block_pty_or_other_engine_requests() {
    let temp = tempfile::tempdir().unwrap();
    let registry = Arc::new(Mutex::new(Registry::new(
        Arc::new(
            ManifestEngine::load_dir(&zeus_engine::detect::bundled_manifest_dir())
                .unwrap()
                .0,
        ),
        temp.path().join("state.json"),
    )));
    let server = Arc::new(
        ControlServer::new(Arc::clone(&registry), temp.path().join("daemon.sock"))
            .with_logs_dir(temp.path().join("logs")),
    );
    let spawned = rpc(&server, "session.spawn", json!({"kind":{"shell":{}},"cwd":"/tmp","initialCols":80,"initialRows":24,"argv":["/bin/sh","-c","stty -echo -icanon min 1 time 0; exec cat"]})).unwrap();
    let id = spawned["id"].as_str().unwrap();
    let (_, owner) = Desktop::open(&server, id);
    let fill = "abcdefghijklmnopqrstuvwxyz0123456789".repeat(100);
    registry
        .lock()
        .unwrap()
        .get(id)
        .unwrap()
        .write_input(fill.as_bytes())
        .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        if registry
            .lock()
            .unwrap()
            .get(id)
            .unwrap()
            .screen_lines()
            .join("")
            .contains("abcdefghijklmnopqrstuvwxyz")
        {
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    // Deliberately never read replies. Sixteen rich-grid snapshots exceed the
    // UDS send buffer, but responses are written after releasing Registry.
    let mut slow = connection(&server);
    let request = ControlMessage::Request {
        id: 1,
        method: SNAPSHOT.into(),
        params: Some(json!({"sessionID":id,"protocol":1})),
    };
    for _ in 0..16 {
        slow.write_all(&zeus_proto::control::encode_line(&request).unwrap())
            .unwrap();
    }
    let start = std::time::Instant::now();
    registry
        .lock()
        .unwrap()
        .get(id)
        .unwrap()
        .write_input(b"\r\nprogress-marker")
        .unwrap();
    loop {
        let snapshot: TerminalSnapshot = serde_json::from_value(
            rpc(&server, SNAPSHOT, json!({"sessionID":id,"protocol":1})).unwrap(),
        )
        .unwrap();
        let text: String = snapshot
            .decode_grid()
            .unwrap()
            .unwrap()
            .changed_rows
            .iter()
            .flat_map(|r| &r.cells)
            .map(|c| char::from_u32(c.scalar).unwrap_or(' '))
            .collect();
        if text.contains("progress-marker") {
            break;
        }
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "slow observer blocked Engine progress"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(start.elapsed() < Duration::from_secs(2));
    drop(slow);
    assert!(rpc(&server, SNAPSHOT, json!({"sessionID":id,"protocol":99})).is_err());
    assert!(
        rpc(
            &server,
            SNAPSHOT,
            json!({"sessionID":id,"protocol":1,"cols":10,"rows":20})
        )
        .is_err()
    );
    assert_eq!(
        rpc(
            &server,
            SCROLLBACK,
            json!({"sessionID":id,"protocol":1,"firstRow":0,"maxRows":129})
        )
        .unwrap_err()
        .code,
        "bad_request"
    );
    assert_eq!(
        owner.control.owner.unwrap().role,
        zeus_proto::ClientRole::Desktop
    );
    rpc(&server, "session.kill", json!({"sessionID":id})).unwrap();
}
