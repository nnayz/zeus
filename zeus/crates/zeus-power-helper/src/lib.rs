//! Inert package contract for the optional issue #70 privileged Helper.
//!
//! Registration, authorization, IPC, safety sensing, and host-power operations
//! are intentionally absent. Ordinary Zeus builds must report this scaffold as
//! unavailable. The macOS binary exists only so release packaging can establish
//! an exact independently signed nested-code and launchd-metadata contract.

#![forbid(unsafe_code)]

pub const HELPER_IDENTIFIER: &str = "com.zeus.zeus.power-helper";
pub const MACH_SERVICE_NAME: &str = HELPER_IDENTIFIER;
pub const PROTOCOL_MAJOR: u16 = zeus_power::wire::CURRENT_PROTOCOL.major;
pub const PROTOCOL_MINOR: u16 = zeus_power::wire::CURRENT_PROTOCOL.minor;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Availability {
    UnavailableScaffoldOnly,
}

pub const fn availability() -> Availability {
    Availability::UnavailableScaffoldOnly
}
