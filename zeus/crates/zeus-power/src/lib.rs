//! Safe, platform-neutral policy core for Zeus power leases.
//!
//! This crate deliberately has no host-power or service-management implementation.
//! Platform code must authenticate peers, collect observations, and implement any
//! privileged effects outside this crate.

#![forbid(unsafe_code)]

pub mod auth;
pub mod eligibility;
pub mod lease;
pub mod recovery;
pub mod wire;
