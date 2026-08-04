//! The control server over a real Unix socket.
//!
//! The unit tests exercise the dispatcher directly; this one goes through the
//! wire: bind, connect, write newline-delimited JSON, read the replies back.
//! A private socket in a temp directory — nothing near the real daemon's.

#![cfg(unix)]

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::{Arc, Mutex};

use diri_engine::control::ControlServer;
use diri_engine::detect::ManifestEngine;
use diri_engine::registry::Registry;
use diri_proto::{ControlMessage, WIRE_VERSION};
use serde_json::json;

fn engine() -> Arc<ManifestEngine> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../Sources/DirijorCore/Resources/manifests")
        .canonicalize()
        .expect("manifests");
    let (engine, _) = ManifestEngine::load_dir(&dir).expect("load");
    Arc::new(engine)
}

#[test]
fn a_client_can_handshake_and_list_over_the_socket() {
    let temp = tempfile::tempdir().expect("temp");
    let registry = Registry::new(engine(), temp.path().join("state.json"));
    let server = Arc::new(ControlServer::new(
        Arc::new(Mutex::new(registry)),
        temp.path().join("daemon.sock"),
    ));
    let listener = server.bind().expect("bind");

    // One connection, served on a thread, the way a daemon would.
    let accepting = {
        let server = Arc::clone(&server);
        std::thread::spawn(move || {
            let (stream, _) = listener.accept().expect("accept");
            let _ = server.serve(stream);
        })
    };

    let client_handle = UnixStream::connect(server.socket_path()).expect("connect");
    let mut client = client_handle.try_clone().expect("clone for writing");
    let mut reader = BufReader::new(client_handle.try_clone().expect("clone for reading"));

    let mut request = |message: ControlMessage| {
        let mut bytes = serde_json::to_vec(&message).expect("encode");
        bytes.push(b'\n');
        client.write_all(&bytes).expect("write");
        client.flush().expect("flush");

        let mut line = String::new();
        reader.read_line(&mut line).expect("read a reply");
        serde_json::from_str::<ControlMessage>(&line).expect("decode")
    };

    let hello = request(ControlMessage::Request {
        id: 1,
        method: "hello".into(),
        params: Some(json!({ "proto": WIRE_VERSION, "build": "integration-test" })),
    });
    match hello {
        ControlMessage::Response {
            id,
            result: Ok(result),
        } => {
            assert_eq!(id, 1, "the reply carries the request's id");
            assert_eq!(result["proto"], WIRE_VERSION);
        }
        other => panic!("handshake failed: {other:?}"),
    }

    let list = request(ControlMessage::Request {
        id: 2,
        method: "session.list".into(),
        params: None,
    });
    match list {
        ControlMessage::Response {
            id,
            result: Ok(result),
        } => {
            assert_eq!(id, 2);
            assert!(result["sessions"].is_array());
        }
        other => panic!("list failed: {other:?}"),
    }

    // A bad request must not take the connection down: the next call still works.
    let bad = request(ControlMessage::Request {
        id: 3,
        method: "session.send_text".into(),
        params: Some(json!({ "id": "s_nope", "text": "hi" })),
    });
    assert!(
        matches!(bad, ControlMessage::Response { result: Err(_), .. }),
        "expected an error reply"
    );

    let after = request(ControlMessage::Request {
        id: 4,
        method: "hello".into(),
        params: Some(json!({ "proto": WIRE_VERSION, "build": "integration-test" })),
    });
    assert!(
        matches!(
            after,
            ControlMessage::Response {
                id: 4,
                result: Ok(_)
            }
        ),
        "the connection should survive an error reply"
    );

    // Shut the write side down explicitly. Dropping `client` is not enough:
    // the BufReader holds a dup of the same socket, so the server would never
    // see EOF and this test would hang on the join.
    client_handle
        .shutdown(std::net::Shutdown::Write)
        .expect("half-close");
    accepting.join().expect("server thread");
}

#[test]
fn spawning_a_shell_over_the_socket_produces_a_watched_session() {
    // The capstone: a client asks the engine to start something, and gets back
    // a session record for a process the engine is now watching.
    let temp = tempfile::tempdir().expect("temp");
    let registry = Registry::new(engine(), temp.path().join("state.json"));
    let registry = Arc::new(Mutex::new(registry));
    let server = ControlServer::new(Arc::clone(&registry), temp.path().join("daemon.sock"))
        .with_logs_dir(temp.path().join("logs"));
    let listener = server.bind().expect("bind");

    let server = Arc::new(server);
    let accepting = {
        let server = Arc::clone(&server);
        std::thread::spawn(move || {
            let (stream, _) = listener.accept().expect("accept");
            let _ = server.serve(stream);
        })
    };

    let client_handle = UnixStream::connect(server.socket_path()).expect("connect");
    let mut client = client_handle.try_clone().expect("clone for writing");
    let mut reader = BufReader::new(client_handle.try_clone().expect("clone for reading"));

    let mut request = |message: ControlMessage| {
        let mut bytes = serde_json::to_vec(&message).expect("encode");
        bytes.push(b'\n');
        client.write_all(&bytes).expect("write");
        client.flush().expect("flush");
        let mut line = String::new();
        reader.read_line(&mut line).expect("read a reply");
        serde_json::from_str::<ControlMessage>(&line).expect("decode")
    };

    let spawned = request(ControlMessage::Request {
        id: 1,
        method: "session.spawn".into(),
        params: Some(json!({
            "kind": "shell",
            "cwd": "/tmp",
            "argv": ["/bin/sh", "-c", "printf spawned-ok\\n; sleep 30"],
        })),
    });
    let id = match spawned {
        ControlMessage::Response {
            result: Ok(result), ..
        } => result["session"]["id"]
            .as_str()
            .expect("a session id")
            .to_string(),
        other => panic!("spawn failed: {other:?}"),
    };
    assert!(id.starts_with("s_"), "id follows the daemon format: {id}");

    // The engine is really watching it: its output reaches the screen.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut seen = false;
    while std::time::Instant::now() < deadline && !seen {
        let screen = request(ControlMessage::Request {
            id: 2,
            method: "session.read_screen".into(),
            params: Some(json!({ "id": id })),
        });
        if let ControlMessage::Response {
            result: Ok(result), ..
        } = screen
        {
            seen = result["lines"].as_array().is_some_and(|lines| {
                lines.iter().any(|line| {
                    line.as_str()
                        .is_some_and(|text| text.contains("spawned-ok"))
                })
            });
        }
        if !seen {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }
    assert!(
        seen,
        "the spawned process's output never reached the engine"
    );

    // And listing reports it.
    let listed = request(ControlMessage::Request {
        id: 3,
        method: "session.list".into(),
        params: None,
    });
    match listed {
        ControlMessage::Response {
            result: Ok(result), ..
        } => {
            let ids: Vec<&str> = result["sessions"]
                .as_array()
                .expect("array")
                .iter()
                .filter_map(|session| session["id"].as_str())
                .collect();
            assert!(
                ids.contains(&id.as_str()),
                "spawned session missing: {ids:?}"
            );
        }
        other => panic!("list failed: {other:?}"),
    }

    let killed = request(ControlMessage::Request {
        id: 4,
        method: "session.kill".into(),
        params: Some(json!({ "id": id })),
    });
    assert!(matches!(
        killed,
        ControlMessage::Response { result: Ok(_), .. }
    ));

    client_handle
        .shutdown(std::net::Shutdown::Write)
        .expect("half-close");
    accepting.join().expect("server thread");
}
