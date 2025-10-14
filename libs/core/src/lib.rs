//! Greentic Messaging core contracts and value types.
//!
//! This crate exposes the shared data structures exchanged between ingress, runner, and egress
//! components. It also provides validation helpers and small utilities for subject naming and
//! idempotency tracking.
pub mod idempotency;
pub mod subjects;
pub mod types;
pub mod validate;

pub use idempotency::*;
pub use subjects::*;
pub use types::*;
pub use validate::*;

/// Returns the semantic version advertised by this crate.
///
/// ```
/// assert_eq!(gsm_core::version(), "0.1.0");
/// ```
pub fn version() -> &'static str {
    "0.1.0"
}
