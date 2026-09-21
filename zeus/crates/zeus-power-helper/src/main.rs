#![forbid(unsafe_code)]

#[cfg(not(target_os = "macos"))]
compile_error!("zeus-power-helper is a macOS-only packaged executable");

#[cfg(target_os = "macos")]
fn main() {
    // Fail closed. This first package milestone deliberately has no listener,
    // registration call, authorization request, journal access, or power API.
    eprintln!(
        "power_helper_unavailable: signed package scaffold only (protocol {}.{})",
        zeus_power_helper::PROTOCOL_MAJOR,
        zeus_power_helper::PROTOCOL_MINOR
    );
    std::process::exit(78);
}
