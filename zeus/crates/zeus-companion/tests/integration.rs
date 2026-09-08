mod support;
use std::time::{Duration, Instant};
use support::Fixture;
use zeus_companion_api::*;
use zeus_companion_client::Client;

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
