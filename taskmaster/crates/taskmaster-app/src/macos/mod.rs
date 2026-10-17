pub mod brand_raster;
pub mod browser;
pub mod menu_bar;
pub mod notifier;
pub mod sf_symbols;

use objc2_foundation::NSBundle;

pub(crate) fn local_profile_name() -> String {
    use objc2::rc::Retained;
    use objc2_foundation::{NSFullUserName, NSString};
    use std::ffi::c_void;

    #[link(name = "SystemConfiguration", kind = "framework")]
    unsafe extern "C" {
        fn SCDynamicStoreCopyComputerName(
            store: *const c_void,
            encoding: *mut u32,
        ) -> *mut NSString;
    }

    // SAFETY: SystemConfiguration returns an owned CFString (+1), which is
    // toll-free bridged to NSString. Retained releases it after conversion.
    let computer_name = unsafe {
        Retained::from_raw(SCDynamicStoreCopyComputerName(
            std::ptr::null(),
            std::ptr::null_mut(),
        ))
    }
    .map(|name| name.to_string());
    let owner = computer_name.as_deref().and_then(computer_owner_name);
    let name = owner
        .unwrap_or_else(|| NSFullUserName().to_string())
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if name.is_empty() {
        return "User".to_owned();
    }
    // Managed accounts may append an organization to the full name.
    let person = name
        .split_once(" - ")
        .map_or(name.as_str(), |(person, _)| person.trim());
    let person = if person.is_empty() {
        name.as_str()
    } else {
        person
    };
    person.chars().take(128).collect::<String>()
}

fn computer_owner_name(computer_name: &str) -> Option<String> {
    let name = computer_name.trim();
    for separator in ["’s ", "'s "] {
        if let Some((owner, _)) = name.split_once(separator)
            && !owner.trim().is_empty()
        {
            return Some(owner.trim().to_owned());
        }
    }
    let lower = name.to_ascii_lowercase();
    if [
        "macbook pro",
        "macbook air",
        "macbook",
        "mac studio",
        "mac mini",
        "imac",
        "mac",
        "pc",
    ]
    .contains(&lower.as_str())
    {
        return None;
    }
    for suffix in [
        " macbook pro",
        " macbook air",
        " macbook",
        " mac studio",
        " mac mini",
        " imac",
        " pc",
    ] {
        if lower.ends_with(suffix) {
            let owner = name[..name.len() - suffix.len()].trim();
            if !owner.is_empty() {
                return Some(owner.to_owned());
            }
        }
    }
    // Inventory names such as PHL-ABC123 do not identify a person; use the
    // local macOS account's full name in that case.
    if !name.is_empty()
        && name
            .chars()
            .all(|character| character.is_alphabetic() || character.is_whitespace())
    {
        Some(name.to_owned())
    } else {
        None
    }
}

pub(crate) fn bundle_identifier() -> Option<String> {
    NSBundle::mainBundle()
        .bundleIdentifier()
        .map(|identifier| identifier.to_string())
}
