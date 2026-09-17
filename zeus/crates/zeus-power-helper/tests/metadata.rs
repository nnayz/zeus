use std::path::Path;

use plist::{Dictionary, Value};
use zeus_power_helper::{HELPER_IDENTIFIER, MACH_SERVICE_NAME};

fn metadata() -> Dictionary {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/com.zeus.zeus.power-helper.plist");
    Value::from_file(path)
        .expect("parse fixed launchd metadata")
        .into_dictionary()
        .expect("metadata dictionary")
}

#[test]
fn launchd_metadata_is_fixed_and_demand_only() {
    let metadata = metadata();
    assert_eq!(
        metadata.get("Label").and_then(Value::as_string),
        Some(HELPER_IDENTIFIER)
    );
    assert_eq!(
        metadata.get("BundleProgram").and_then(Value::as_string),
        Some("Contents/Library/HelperTools/com.zeus.zeus.power-helper")
    );
    let services = metadata
        .get("MachServices")
        .and_then(Value::as_dictionary)
        .expect("fixed Mach service");
    assert_eq!(services.len(), 1);
    assert_eq!(
        services.get(MACH_SERVICE_NAME).and_then(Value::as_boolean),
        Some(true)
    );
    assert_eq!(
        metadata.get("RunAtLoad").and_then(Value::as_boolean),
        Some(false)
    );
    assert_eq!(
        metadata.get("KeepAlive").and_then(Value::as_boolean),
        Some(false)
    );
}

#[test]
fn metadata_has_no_command_or_ambient_authority_surface() {
    let metadata = metadata();
    for forbidden in [
        "Program",
        "ProgramArguments",
        "EnvironmentVariables",
        "Sockets",
        "UserName",
        "GroupName",
        "WatchPaths",
        "QueueDirectories",
        "StartInterval",
        "StartCalendarInterval",
    ] {
        assert!(!metadata.contains_key(forbidden), "unexpected {forbidden}");
    }
}
