//! Regression guards for the intentionally non-privileged issue #70 spike.
//!
//! These tests are tripwires, not a security proof. An approved privileged lab
//! phase must replace them deliberately rather than silently growing a command
//! or service-registration seam inside this mock-only crate.

use std::fs;
use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("zeus-power lives at <workspace>/crates/zeus-power")
        .to_path_buf()
}

fn files_below(root: &Path, extensions: &[&str]) -> Vec<PathBuf> {
    fn visit(path: &Path, extensions: &[&str], files: &mut Vec<PathBuf>) {
        let mut entries: Vec<_> = fs::read_dir(path)
            .unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
            .map(|entry| entry.expect("directory entry").path())
            .collect();
        entries.sort();
        for entry in entries {
            if entry.is_dir() {
                visit(&entry, extensions, files);
            } else if extensions.is_empty()
                || entry
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .is_some_and(|extension| extensions.contains(&extension))
            {
                files.push(entry);
            }
        }
    }

    let mut files = Vec::new();
    visit(root, extensions, &mut files);
    files
}

fn joined(parts: &[&str]) -> String {
    parts.concat()
}

#[test]
fn policy_core_has_no_process_ffi_or_binary_seam() {
    let crate_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let source_root = crate_root.join("src");
    let lib = fs::read_to_string(source_root.join("lib.rs")).expect("read lib.rs");
    assert!(
        lib.contains("#![forbid(unsafe_code)]"),
        "the mock-only policy core must continue to forbid unsafe code"
    );

    let forbidden = [
        joined(&["std::", "process"]),
        joined(&["tokio::", "process"]),
        joined(&["Command", "::new"]),
        joined(&["libc", "::"]),
        joined(&["unsafe", " {"]),
        joined(&["unsafe", " fn"]),
        joined(&["extern ", "\"C\""]),
        joined(&["extern ", "\"system\""]),
    ];
    for path in files_below(&source_root, &["rs"]) {
        let source = fs::read_to_string(&path).expect("read Rust source");
        for token in &forbidden {
            assert!(
                !source.contains(token),
                "{} introduces forbidden process/FFI seam `{token}`",
                path.display()
            );
        }
    }

    for forbidden_path in [
        crate_root.join("build.rs"),
        source_root.join("main.rs"),
        source_root.join("bin"),
    ] {
        assert!(
            !forbidden_path.exists(),
            "mock-only zeus-power must remain library-only: {}",
            forbidden_path.display()
        );
    }
}

#[test]
fn workspace_has_no_power_mutation_or_service_registration_code() {
    let root = workspace_root();
    let forbidden = [
        joined(&["pm", "set"]),
        joined(&["Sleep", "Disabled"]),
        joined(&["SMApp", "Service"]),
        joined(&["SMJob", "Bless"]),
        joined(&["Service", "Management"]),
        joined(&["AuthorizationExecute", "WithPrivileges"]),
        joined(&["kSMRightBless", "PrivilegedHelper"]),
    ];

    for source_root in [root.join("crates"), root.join("scripts")] {
        for path in files_below(&source_root, &["rs", "sh", "plist"]) {
            let source = fs::read_to_string(&path).expect("read production source");
            for token in &forbidden {
                assert!(
                    !source.contains(token),
                    "{} introduces forbidden privileged token `{token}`",
                    path.display()
                );
            }
        }
    }
}

#[test]
fn no_power_helper_artifact_is_present() {
    let root = workspace_root();
    let helper_name = joined(&["com.zeus.zeus.", "power-helper"]);
    let mut artifacts = Vec::new();
    for path in files_below(&root.join("crates"), &[]) {
        if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name == helper_name)
        {
            artifacts.push(path);
        }
    }
    assert!(
        artifacts.is_empty(),
        "privileged helper artifacts require separate written approval: {artifacts:?}"
    );
}
