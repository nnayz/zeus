//! Regression tests for the external trust boundary, independent of Engine DTOs.
use serde_json::json;
use zeus_companion_api::{
    AcquireControl, Mutation, PairRequest, ReleaseControl, SendText, bounded_string, valid_id,
};

#[test]
fn a_phone_cannot_assign_its_own_scopes_or_skip_server_verification() {
    let pair = json!({
        "api_major": 1,
        "expected_server_id": "expected-server",
        "code": "one-time-code",
        "device_name": "Phone",
    });
    assert!(serde_json::from_value::<PairRequest>(pair.clone()).is_ok());
    let mut extra_scopes = pair.clone();
    extra_scopes["scopes"] = json!(["lifecycle", "spawn"]);
    assert!(serde_json::from_value::<PairRequest>(extra_scopes).is_err());
    let mut missing_identity = pair;
    missing_identity
        .as_object_mut()
        .unwrap()
        .remove("expected_server_id");
    assert!(serde_json::from_value::<PairRequest>(missing_identity).is_err());
}

#[test]
fn controller_identity_is_not_supplied_by_the_phone() {
    let epoch = json!({"incarnation": "session-incarnation", "generation": 7});
    let acquire = json!({"expected": epoch, "takeover": true});
    assert!(serde_json::from_value::<AcquireControl>(acquire.clone()).is_ok());
    let mut spoofed = acquire;
    spoofed["owner"] = json!({"id": "another-device", "role": "desktop"});
    assert!(serde_json::from_value::<AcquireControl>(spoofed).is_err());
    assert!(
        serde_json::from_value::<ReleaseControl>(json!({
            "expected": epoch,
            "owner_id": "another-device",
        }))
        .is_err()
    );
    assert!(
        serde_json::from_value::<SendText>(json!({
            "expected": epoch, "command_seq": 1, "text": "hello", "submit": true,
            "owner_id": "another-device",
        }))
        .is_err()
    );
}

#[test]
fn mutation_allowlist_rejects_internal_rpc_and_execution_arguments() {
    let mutation = json!({
        "engine_epoch": "engine-incarnation",
        "mutation_id": "mutation-00000001",
        "expected_revision": "revision-1",
        "action": {"kind": "rename", "title": "Title"},
    });
    assert!(serde_json::from_value::<Mutation>(mutation.clone()).is_ok());
    for action in [
        json!({"kind": "daemon.shutdown"}),
        json!({"kind": "test.run", "argv": ["sh", "-c", "anything"]}),
        json!({"kind": "rename", "title": "Title", "environment": {"SECRET": "value"}}),
        json!({"kind": "terminate"}),
    ] {
        let mut invalid = mutation.clone();
        invalid["action"] = action;
        assert!(serde_json::from_value::<Mutation>(invalid).is_err());
    }
    let mut rpc = mutation;
    rpc["method"] = json!("session.capture_env");
    assert!(serde_json::from_value::<Mutation>(rpc).is_err());
}

#[test]
fn command_sequences_reject_negative_fractional_and_overflowing_json_numbers() {
    for sequence in ["-1", "1.5", "18446744073709551616", "null", "\"1\""] {
        let request = format!(
            r#"{{"expected":{{"incarnation":"session","generation":1}},"command_seq":{sequence},"text":"hello","submit":true}}"#
        );
        assert!(serde_json::from_str::<SendText>(&request).is_err());
    }
}

#[test]
fn identifiers_cannot_be_paths_or_encoded_path_components() {
    for value in [
        "",
        "../session",
        "session/id",
        "%2F",
        "s?token=x",
        "s#fragment",
        "s\0",
        "sü",
    ] {
        assert!(
            !valid_id(value),
            "unexpected accepted identifier: {value:?}"
        );
    }
    assert!(valid_id("s_session-1"));
    assert!(valid_id(&"a".repeat(64)));
    assert!(!valid_id(&"a".repeat(65)));
}

#[test]
fn projection_truncation_preserves_utf8_at_each_byte_boundary() {
    let value = "aé🦀z";
    for limit in 0..=value.len() + 1 {
        let bounded = bounded_string(value, limit);
        assert!(bounded.len() <= limit);
        assert!(value.starts_with(&bounded));
        let next = value[bounded.len()..].chars().next();
        assert!(next.is_none_or(|next| bounded.len() + next.len_utf8() > limit));
    }
}

#[test]
fn mutation_diagnostics_redact_user_supplied_titles() {
    let mutation: Mutation = serde_json::from_value(json!({
        "engine_epoch": "engine-incarnation", "mutation_id": "mutation-00000001",
        "expected_revision": "revision-1",
        "action": {"kind": "rename", "title": "private prompt or credential"},
    }))
    .unwrap();
    assert!(!format!("{mutation:?}").contains("private prompt or credential"));
}
