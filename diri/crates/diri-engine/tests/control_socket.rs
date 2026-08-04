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
