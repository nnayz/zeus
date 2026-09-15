//! Safe, platform-neutral policy core for Zeus power leases.
//!
//! This crate deliberately has no host-power or service-management implementation.
//! External platform adapters may authenticate peers and collect observations, but
//! raw privileged mutation stays sealed in this exploration. If a production seam
//! is approved, its backend must live behind a checked facade in this crate rather
//! than expose unrestricted power-setting methods to callers.

#![forbid(unsafe_code)]

pub mod auth;
pub mod eligibility;
pub mod lease;
pub mod recovery;
pub mod wire;
