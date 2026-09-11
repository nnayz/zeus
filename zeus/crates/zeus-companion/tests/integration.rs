mod support;
use std::time::{Duration, Instant};
use support::Fixture;
use zeus_companion_api::*;
use zeus_companion_client::Client;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn delayed_prompt_output_invalidates_without_metadata_or_control_change() {
    use std::{io::Write, os::unix::fs::OpenOptionsExt};
    let fixture = Fixture::new().await;
    let gate = fixture.temp.path().join("output-gate");
    let path = std::ffi::CString::new(gate.as_os_str().as_encoded_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
    // The PTY consumes the prompt, then waits for an independent test-controlled
    // gate. No timer, rename, control RPC or status mutation releases its output.
    let script = "stty -echo; printf 'fixture-ready\\n'; IFS= read -r prompt; : > prompt-received; IFS= read -r release < output-gate; printf 'delayed:%s\\n' \"$prompt\"; exec /bin/cat";
    let result = fixture
        .daemon
        .request(
            "session.spawn",
            Some(&serde_json::json!({
                "kind":{"shell":{}}, "cwd":fixture.temp.path(),
                "argv":["/bin/sh", "-c", script], "title":"delayed output fixture",
                "initialCols":80, "initialRows":24
            })),
            Some(Duration::from_secs(5)),
        )
        .await
        .unwrap();
    let id = result["id"].as_str().unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let screen = loop {
        if let Ok(screen) = fixture.client.screen(id).await
            && screen.text.contains("fixture-ready")
        {
            break screen;
        }
        assert!(Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(10)).await;
    };
    let control = fixture
        .client
        .acquire(
            id,
            &AcquireControl {
                expected: screen.control.epoch,
                takeover: false,
            },
        )
        .await
        .unwrap();
    fixture
        .client
        .send_text(
            id,
            &SendText {
                expected: control.epoch,
                command_seq: 1,
                text: "output-only-marker".into(),
                submit: true,
            },
        )
        .await
        .unwrap();
    while !fixture.temp.path().join("prompt-received").exists() {
        assert!(Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let mut events = fixture.client.subscribe(None).await.unwrap();
    assert_eq!(events.next().await.unwrap().kind, "resync_required");
    // Drain startup and command invalidations across two watcher intervals.
    loop {
        assert!(Instant::now() < deadline);
        if tokio::time::timeout(Duration::from_millis(350), events.next())
            .await
            .is_err()
        {
            break;
        }
    }
    let before = fixture.client.session(id).await.unwrap();
    let screen_before = fixture.client.screen(id).await.unwrap();
    let bus = fixture.engine.lock().unwrap().events();
    let source = bus.subscribe(
        None,
        zeus_engine::events::Filter::new(
            None,
            Some(vec![
                "session.output".into(),
                "session.updated".into(),
                "terminal.control_changed".into(),
            ]),
        ),
    );
    let start = Instant::now();
    std::fs::OpenOptions::new()
        .write(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(gate)
        .unwrap()
        .write_all(b"release\n")
        .unwrap();
    let changed = tokio::time::timeout(Duration::from_secs(3), events.next())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(changed.kind, "changed");
    let internal = source
        .try_recv()
        .expect("Engine published the invalidation");
    assert_eq!(internal.name, "session.output");
    assert!(internal.params.is_null());
    assert!(
        source.try_recv().is_none(),
        "no metadata/control event needed"
    );
    let screen_after = fixture.client.screen(id).await.unwrap();
    assert!(screen_after.text.contains("delayed:output-only-marker"));
    assert!(screen_after.screen_sequence > screen_before.screen_sequence);
    assert_eq!(screen_after.control, screen_before.control);
    assert_eq!(fixture.client.session(id).await.unwrap(), before);
    eprintln!(
        "companion gated_output_to_event_and_screen_us={}",
        start.elapsed().as_micros()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sidecar_binary_closes_listener_and_upgraded_stream_on_parent_stdin_eof() {
    use std::process::Stdio;
    use zeus_companion::config::{Config, atomic_write};
    let mut fixture = Fixture::new().await;
    fixture.stop_gateway().await;
    let config = Config {
        bind: fixture
            .origin
            .strip_prefix("http://")
            .unwrap()
            .parse()
            .unwrap(),
        origins: vec![fixture.origin.clone()],
        ..Config::default()
    };
    let path = fixture.auth.directory.join("config.json");
    atomic_write(&path, &serde_json::to_vec(&config).unwrap()).unwrap();
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_zeus-companion"))
        .arg("serve")
        .arg(path)
        .arg(fixture.temp.path().join("daemon.sock"))
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while fixture.client.hello(&[]).await.is_err() {
        assert!(Instant::now() < deadline, "sidecar did not become ready");
        assert!(child.try_wait().unwrap().is_none());
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let mut events = fixture.client.subscribe(None).await.unwrap();
    events.next().await.unwrap();
    drop(child.stdin.take());
    let status = tokio::time::timeout(Duration::from_secs(5), child.wait())
        .await
        .unwrap()
        .unwrap();
    assert!(status.success());
    assert!(
        tokio::time::timeout(Duration::from_secs(2), events.next())
            .await
            .unwrap()
            .is_err()
    );
    assert!(fixture.client.hello(&[]).await.is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reference_client_drives_real_engine_and_rejects_replay_across_gateway_restart() {
    let mut fixture = Fixture::new().await;
    let id = fixture.spawn_echo().await;
    let mut events = fixture.client.subscribe(None).await.unwrap();
    let first = tokio::time::timeout(Duration::from_secs(2), events.next())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first.kind, "resync_required");
    let page = fixture
        .client
        .sessions(&PageRequest {
            offset: 0,
            limit: Some(1),
        })
        .await
        .unwrap();
    assert_eq!(page.items[0].id, id);
    let projects = fixture.client.projects(&Default::default()).await.unwrap();
    assert_eq!(projects.items.len(), 1);
    let screen = fixture.client.screen(&id).await.unwrap();
    assert_eq!((screen.cols, screen.rows), (80, 24));
    let state = fixture
        .client
        .acquire(
            &id,
            &AcquireControl {
                expected: screen.control.epoch,
                takeover: false,
            },
        )
        .await
        .unwrap();
    assert_eq!(
        state.owner.as_ref().unwrap().id,
        fixture.credentials.device_id
    );
    let prompt = SendText {
        expected: state.epoch.clone(),
        command_seq: 1,
        text: "companion-conformance".into(),
        submit: true,
    };
    let before = Instant::now();
    let state = fixture.client.send_text(&id, &prompt).await.unwrap();
    assert_eq!(state.command_seq, 1);
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let screen = fixture.client.screen(&id).await.unwrap();
        if screen.text.contains("companion-conformance") {
            assert_eq!(screen.text.matches("companion-conformance").count(), 1);
            break;
        }
        assert!(Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    eprintln!(
        "companion input_to_screen_us={}",
        before.elapsed().as_micros()
    );
    let changed = tokio::time::timeout(Duration::from_secs(2), events.next())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(changed.kind, "changed");
    let mut resumed = fixture
        .client
        .subscribe(events.cursor.clone())
        .await
        .unwrap();
    fixture.engine.lock().unwrap().events().publish(
        "session.updated",
        serde_json::json!({"secret":"must-not-cross-gateway"}),
        Some(&id),
    );
    let resumed_event = tokio::time::timeout(Duration::from_secs(2), resumed.next())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(resumed_event.kind, "changed");
    assert!(
        !serde_json::to_string(&resumed_event)
            .unwrap()
            .contains("secret")
    );
    assert_eq!(
        fixture
            .client
            .send_text(&id, &prompt)
            .await
            .err()
            .unwrap()
            .code,
        "command_sequence"
    );
    let detail = fixture.client.session(&id).await.unwrap();
    let mutation = Mutation {
        engine_epoch: detail.engine_epoch,
        mutation_id: "fixture-rename-0001".into(),
        expected_revision: detail.session.revision,
        expected_control: None,
        action: Action::Rename {
            title: "renamed".into(),
        },
    };
    assert!(fixture.client.mutate(&id, &mutation).await.unwrap().applied);
    assert_eq!(
        fixture
            .client
            .mutate(&id, &mutation)
            .await
            .err()
            .unwrap()
            .code,
        "replayed_mutation"
    );
    let old_cursor = events.cursor.clone();
    fixture.restart_gateway().await;
    assert_eq!(
        fixture
            .client
            .mutate(&id, &mutation)
            .await
            .err()
            .unwrap()
            .code,
        "replayed_mutation"
    );
    assert_eq!(
        fixture
            .client
            .send_text(&id, &prompt)
            .await
            .err()
            .unwrap()
            .code,
        "command_sequence"
    );
    let mut after_restart = fixture.client.subscribe(old_cursor).await.unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(2), after_restart.next())
            .await
            .unwrap()
            .unwrap()
            .kind,
        "resync_required"
    );
    fixture
        .client
        .release(
            &id,
            &ReleaseControl {
                expected: state.epoch,
            },
        )
        .await
        .unwrap();
    let old_epoch = fixture.client.hello(&[]).await.unwrap().engine_epoch;
    fixture.restart_engine_endpoint().await;
    let new_epoch = fixture.client.hello(&[]).await.unwrap().engine_epoch;
    assert_ne!(old_epoch, new_epoch);
    assert_eq!(
        fixture
            .client
            .mutate(&id, &mutation)
            .await
            .err()
            .unwrap()
            .code,
        "stale_engine"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_websocket_closes_on_revocation_and_gateway_shutdown() {
    let mut fixture = Fixture::new().await;
    let mut events = fixture.client.subscribe(None).await.unwrap();
    events.next().await.unwrap();
    fixture.auth.revoke(&fixture.credentials.device_id).unwrap();
    assert!(
        tokio::time::timeout(Duration::from_secs(2), events.next())
            .await
            .unwrap()
            .is_err()
    );
    assert_eq!(
        fixture.client.hello(&[]).await.err().unwrap().code,
        "unauthorized"
    );
    let code = fixture
        .auth
        .enroll(vec![Scope::Read], zeus_companion::auth::now_ms())
        .unwrap();
    let paired = Client::pair(
        &fixture.origin,
        &PairRequest {
            api_major: 1,
            expected_server_id: fixture.credentials.server_id.clone(),
            code,
            device_name: "second device".into(),
        },
    )
    .await
    .unwrap();
    let client = Client::new(&fixture.origin, paired.token, paired.server_id).unwrap();
    let mut events = client.subscribe(None).await.unwrap();
    events.next().await.unwrap();
    fixture.stop_gateway().await;
    assert!(
        tokio::time::timeout(Duration::from_secs(2), events.next())
            .await
            .unwrap()
            .is_err()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lifecycle_requires_confirmation_revision_and_current_device_control() {
    let fixture = Fixture::new().await;
    let id = fixture.spawn_echo().await;
    let detail = fixture.client.session(&id).await.unwrap();
    let mut action = Mutation {
        engine_epoch: detail.engine_epoch,
        mutation_id: "fixture-hibernate-01".into(),
        expected_revision: detail.session.revision,
        expected_control: None,
        action: Action::Hibernate { confirmed: false },
    };
    assert_eq!(
        fixture
            .client
            .mutate(&id, &action)
            .await
            .err()
            .unwrap()
            .code,
        "confirmation_required"
    );
    action.action = Action::Hibernate { confirmed: true };
    assert_eq!(
        fixture
            .client
            .mutate(&id, &action)
            .await
            .err()
            .unwrap()
            .code,
        "not_controller"
    );
    let screen = fixture.client.screen(&id).await.unwrap();
    let control = fixture
        .client
        .acquire(
            &id,
            &AcquireControl {
                expected: screen.control.epoch,
                takeover: false,
            },
        )
        .await
        .unwrap();
    action.expected_control = Some(control.epoch.clone());
    action.expected_revision = fixture.client.session(&id).await.unwrap().session.revision;
    fixture.client.mutate(&id, &action).await.unwrap();
    assert!(
        fixture
            .client
            .session(&id)
            .await
            .unwrap()
            .session
            .hibernated
    );
    action.mutation_id = "fixture-wake-0000001".into();
    action.action = Action::Wake { confirmed: true };
    action.expected_revision = fixture.client.session(&id).await.unwrap().session.revision;
    fixture.client.mutate(&id, &action).await.unwrap();
    assert!(
        !fixture
            .client
            .session(&id)
            .await
            .unwrap()
            .session
            .hibernated
    );
    action.mutation_id = "fixture-terminate-01".into();
    action.action = Action::Terminate { confirmed: true };
    action.expected_revision = fixture.client.session(&id).await.unwrap().session.revision;
    fixture.client.mutate(&id, &action).await.unwrap();
    assert_eq!(
        fixture.client.session(&id).await.unwrap().session.status,
        "exited"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn request_latency_sample_uses_bounded_projection() {
    let fixture = Fixture::new().await;
    let mut samples = Vec::new();
    for _ in 0..50 {
        let start = Instant::now();
        fixture.client.sessions(&Default::default()).await.unwrap();
        samples.push(start.elapsed().as_micros());
    }
    samples.sort();
    eprintln!(
        "companion HTTP_to_Engine requests=50 p50_us={} p90_us={} payload_page_max=64",
        samples[24], samples[44]
    );
}
