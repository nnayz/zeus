mod support;
use futures::{SinkExt, StreamExt};
use std::{
    os::unix::fs::{DirBuilderExt, PermissionsExt, symlink},
    time::Duration,
};
use support::Fixture;
use zeus_companion::{
    auth::AuthStore,
    config::{Config, atomic_write, secure_dir, secure_read, validate_bind},
};
use zeus_companion_api::*;

fn auth() -> (tempfile::TempDir, AuthStore) {
    let dir = tempfile::tempdir_in(std::fs::canonicalize("/tmp").unwrap()).unwrap();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let store = AuthStore {
        directory: dir.path().into(),
    };
    store.initialize().unwrap();
    (dir, store)
}

#[test]
fn pairing_is_server_bound_one_time_expiring_and_scoped() {
    let (_temp, auth) = auth();
    let code = auth.enroll(vec![Scope::Read], 1000).unwrap();
    let id = auth.server_id().unwrap();
    assert!(auth.pair(&code, "phone", &"x".repeat(64), 1001).is_err());
    let paired = auth.pair(&code, "phone", &id, 1002).unwrap();
    assert!(auth.pair(&code, "phone", &id, 1003).is_err());
    assert!(auth.authenticate(&paired.token, Scope::Read, 1004).is_ok());
    assert!(
        auth.authenticate(&paired.token, Scope::Interact, 1004)
            .is_err()
    );
    assert!(
        auth.authenticate(&paired.token, Scope::Read, paired.expires_at_ms)
            .is_err()
    );
    let expired = auth.enroll(vec![Scope::Read], 2000).unwrap();
    assert!(auth.pair(&expired, "phone", &id, 302000).is_err());
    let state =
        String::from_utf8(secure_read(&auth.directory.join("devices.json"), 65536).unwrap())
            .unwrap();
    assert!(!state.contains(&paired.token));
    assert!(!state.contains(&code));
    auth.revoke(&paired.device_id).unwrap();
    assert!(auth.authenticate(&paired.token, Scope::Read, 1004).is_err());
}

#[test]
fn bind_validation_rejects_public_wildcard_mapped_link_local_and_implicit_private() {
    for addr in [
        "0.0.0.0:1",
        "[::]:1",
        "8.8.8.8:1",
        "[2606:4700::1]:1",
        "[::ffff:127.0.0.1]:1",
        "169.254.1.1:1",
        "[fe80::1]:1",
    ] {
        assert!(
            validate_bind(addr.parse().unwrap(), true).is_err(),
            "{addr}"
        );
    }
    for addr in [
        "10.0.0.1:1",
        "192.168.1.1:1",
        "172.16.0.1:1",
        "100.64.0.1:1",
        "[fd01::1]:1",
    ] {
        assert!(validate_bind(addr.parse().unwrap(), false).is_err());
        assert!(validate_bind(addr.parse().unwrap(), true).is_ok());
        let config = Config {
            bind: addr.parse().unwrap(),
            allow_private: true,
            ..Config::default()
        };
        assert!(config.validate().is_err());
    }
    assert!(Config::default().validate().is_ok());
    assert!(serde_json::from_str::<Config>(r#"{"bind":"localhost:19773"}"#).is_err());
}

#[test]
fn secure_files_reject_symlinks_hardlinks_modes_special_files_and_writable_ancestors() {
    let (temp, auth) = auth();
    let path = auth.directory.join("data.json");
    atomic_write(&path, b"fixture").unwrap();
    let link = auth.directory.join("link");
    symlink(&path, &link).unwrap();
    assert!(secure_read(&link, 1024).is_err());
    assert!(atomic_write(&link, b"replacement").is_err());
    let missing = auth.directory.join("missing");
    let dangling = auth.directory.join("dangling");
    symlink(&missing, &dangling).unwrap();
    assert!(atomic_write(&dangling, b"replacement").is_err());
    assert_eq!(std::fs::read_link(&dangling).unwrap(), missing);
    assert!(!missing.exists());
    let hard = auth.directory.join("hard");
    std::fs::hard_link(&path, &hard).unwrap();
    assert!(secure_read(&path, 1024).is_err());
    std::fs::remove_file(hard).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(secure_read(&path, 1024).is_err());
    let parent = temp.path().join("unsafe");
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&parent)
        .unwrap();
    let child = parent.join("state");
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&child)
        .unwrap();
    std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o777)).unwrap();
    assert!(secure_dir(&child).is_err());
    let fifo = auth.directory.join("fifo");
    let c = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
    assert!(secure_read(&fifo, 1024).is_err());
}

#[test]
fn auth_lock_rejects_replaced_symlinks_hardlinks_special_files_and_permissions() {
    for kind in [
        "symlink",
        "dangling",
        "hardlink",
        "fifo",
        "directory",
        "mode",
    ] {
        let (_temp, auth) = auth();
        let lock = auth.directory.join("auth.lock");
        std::fs::remove_file(&lock).unwrap();
        let sentinel = auth.directory.join("sentinel");
        atomic_write(&sentinel, b"unchanged").unwrap();
        match kind {
            "symlink" => symlink(&sentinel, &lock).unwrap(),
            "dangling" => symlink(auth.directory.join("missing"), &lock).unwrap(),
            "hardlink" => std::fs::hard_link(&sentinel, &lock).unwrap(),
            "fifo" => {
                let name = std::ffi::CString::new(lock.as_os_str().as_encoded_bytes()).unwrap();
                assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
            }
            "directory" => std::fs::create_dir(&lock).unwrap(),
            "mode" => {
                atomic_write(&lock, b"").unwrap();
                std::fs::set_permissions(&lock, std::fs::Permissions::from_mode(0o644)).unwrap();
            }
            _ => unreachable!(),
        }
        assert!(auth.server_id().is_err(), "{kind}");
        assert_eq!(std::fs::read(&sentinel).unwrap(), b"unchanged");
        assert!(!auth.directory.join("missing").exists());
    }
}

#[test]
fn restrictive_umask_failure_cleans_up_only_new_temporary_state() {
    use std::os::unix::process::CommandExt;
    let (_temp, auth) = auth();
    let config = auth.directory.join("config.json");
    atomic_write(&config, &serde_json::to_vec(&Config::default()).unwrap()).unwrap();
    let original = secure_read(&auth.directory.join("devices.json"), 65536).unwrap();
    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_zeus-companion"));
    command
        .arg("enroll")
        .arg(config)
        .arg(auth.directory.join("enrollment.json"))
        .arg("read")
        .arg("https://companion.example");
    // Change umask only in the isolated child, never in the parallel test runner.
    unsafe {
        command.pre_exec(|| {
            libc::umask(0o777);
            Ok(())
        });
    }
    assert!(!command.output().unwrap().status.success());
    assert_eq!(
        secure_read(&auth.directory.join("devices.json"), 65536).unwrap(),
        original
    );
    let mut names: Vec<_> = std::fs::read_dir(&auth.directory)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    names.sort();
    assert_eq!(names, ["auth.lock", "config.json", "devices.json"]);
}

#[test]
fn cli_init_validates_before_creation_and_supports_separate_safe_enrollment_output() {
    let temp = tempfile::tempdir_in(std::fs::canonicalize("/tmp").unwrap()).unwrap();
    let outside = tempfile::tempdir_in(std::fs::canonicalize("/tmp").unwrap()).unwrap();
    std::fs::set_permissions(outside.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let alias = temp.path().join("alias");
    symlink(outside.path(), &alias).unwrap();
    let init = |path: &std::path::Path| {
        std::process::Command::new(env!("CARGO_BIN_EXE_zeus-companion"))
            .current_dir(temp.path())
            .arg("init")
            .arg(path)
            .output()
            .unwrap()
            .status
    };
    assert!(!init(std::path::Path::new("relative")).success());
    assert!(!temp.path().join("relative").exists());
    assert!(!init(&temp.path().join("alias/new-state")).success());
    assert!(!outside.path().join("new-state").exists());
    let existing = temp.path().join("existing");
    std::fs::create_dir(&existing).unwrap();
    assert!(!init(&existing.join("../escaped")).success());
    assert!(!temp.path().join("escaped").exists());

    let state = temp.path().join("valid-state");
    assert!(init(&state).success());
    secure_dir(&state).unwrap();
    let output = outside.path().join("enrollment.json");
    let result = std::process::Command::new(env!("CARGO_BIN_EXE_zeus-companion"))
        .arg("enroll")
        .arg(state.join("config.json"))
        .arg(&output)
        .arg("read")
        .arg("https://companion.example")
        .output()
        .unwrap();
    assert!(result.status.success());
    assert!(result.stdout.is_empty());
    assert!(result.stderr.is_empty());
    let payload: serde_json::Value =
        serde_json::from_slice(&secure_read(&output, 4096).unwrap()).unwrap();
    assert_eq!(payload["origin"], "https://companion.example");
    // An HTTP-controlled display name stays JSON data, not a path component.
    let auth = AuthStore { directory: state };
    let paired = auth
        .pair(
            payload["code"].as_str().unwrap(),
            "../outside",
            payload["server_id"].as_str().unwrap(),
            zeus_companion::auth::now_ms(),
        )
        .unwrap();
    assert_eq!(
        auth.authenticate(&paired.token, Scope::Read, zeus_companion::auth::now_ms())
            .unwrap()
            .name,
        "../outside"
    );
    assert!(!temp.path().join("outside").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn http_allowlist_auth_origins_payload_and_pair_version_fail_closed() {
    let fixture = Fixture::new().await;
    let http = reqwest::Client::builder().no_proxy().build().unwrap();
    assert_eq!(
        http.get(format!("{}/v1/sessions", fixture.origin))
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    for path in [
        "/rpc",
        "/v1/rpc",
        "/v1/daemon/shutdown",
        "/v1/test/run",
        "/v2/hello",
        "/v1/sessions/a/resize",
        "/v1/sessions/a/signal",
    ] {
        assert_eq!(
            http.post(format!("{}{path}", fixture.origin))
                .bearer_auth(&fixture.credentials.token)
                .json(&serde_json::json!({"method":"daemon.shutdown"}))
                .send()
                .await
                .unwrap()
                .status(),
            404
        );
    }
    assert_eq!(
        http.get(format!("{}/v1/sessions", fixture.origin))
            .bearer_auth(&fixture.credentials.token)
            .header("Origin", "https://attacker.invalid")
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    let cors = http
        .get(format!("{}/v1/sessions", fixture.origin))
        .bearer_auth(&fixture.credentials.token)
        .header("Origin", &fixture.origin)
        .send()
        .await
        .unwrap();
    assert_eq!(
        cors.headers()["access-control-allow-origin"],
        fixture.origin
    );
    assert_eq!(cors.headers()["cache-control"], "no-store");
    let oversized = http
        .post(format!("{}/v1/sessions/a/actions", fixture.origin))
        .bearer_auth(&fixture.credentials.token)
        .header("Content-Type", "application/json")
        .body("a".repeat(4097))
        .send()
        .await
        .unwrap();
    assert_eq!(oversized.status(), 413);
    assert!(!oversized.text().await.unwrap().contains("aaaa"));
    let version = http
        .post(format!("{}/v1/pair", fixture.origin))
        .json(&PairRequest {
            api_major: 2,
            expected_server_id: fixture.credentials.server_id.clone(),
            code: "0".repeat(64),
            device_name: "phone".into(),
        })
        .send()
        .await
        .unwrap();
    assert_eq!(version.status(), 426);
    let unknown_scope=http.post(format!("{}/v1/pair",fixture.origin)).json(&serde_json::json!({"api_major":1,"expected_server_id":fixture.credentials.server_id,"code":"0".repeat(64),"device_name":"phone","scopes":["lifecycle"]})).send().await.unwrap();
    assert_eq!(unknown_scope.status(), 400);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn websocket_auth_protocol_malformed_oversized_and_admission_are_bounded() {
    use tokio_tungstenite::{connect_async, tungstenite::Message};
    let fixture = Fixture::new().await;
    let url = fixture.origin.replace("http://", "ws://") + "/v1/events";
    for frame in [
        "not-json".to_owned(),
        serde_json::to_string(&Subscribe {
            api_major: 2,
            token: fixture.credentials.token.clone(),
            cursor: None,
        })
        .unwrap(),
        serde_json::to_string(&Subscribe {
            api_major: 1,
            token: "0".repeat(64),
            cursor: None,
        })
        .unwrap(),
        "x".repeat(2049),
    ] {
        let (mut ws, _) = connect_async(&url).await.unwrap();
        ws.send(Message::Text(frame.into())).await.unwrap();
        let response = tokio::time::timeout(Duration::from_secs(2), ws.next())
            .await
            .unwrap();
        assert!(!matches!(response, Some(Ok(Message::Text(_)))));
    }
    let mut held = Vec::new();
    for _ in 0..8 {
        let mut stream = fixture.client.subscribe(None).await.unwrap();
        stream.next().await.unwrap();
        held.push(stream);
    }
    assert!(fixture.client.subscribe(None).await.is_err());
    // Saturation of the websocket quota never stalls a normal Engine projection.
    fixture.client.sessions(&Default::default()).await.unwrap();
}

#[test]
fn replay_overflow_and_stream_restart_require_a_fresh_projection() {
    let mut hub = zeus_companion::server::EventHub::new().unwrap();
    let initial = hub.replay(None)[0].cursor.clone();
    for _ in 0..=EVENT_WINDOW {
        hub.publish("changed");
    }
    assert_eq!(hub.replay(Some(&initial))[0].kind, "resync_required");
    let latest = hub.replay(None)[0].cursor.clone();
    assert!(hub.replay(Some(&latest)).is_empty());
    let next = zeus_companion::server::EventHub::new().unwrap();
    assert_eq!(next.replay(Some(&latest))[0].kind, "resync_required");
}
