#![doc = "Pure domain types and planning rules for SonicMux."]
#![forbid(unsafe_code)]

pub mod error;

pub use error::CoreError;

/// The package name, exposed for workspace smoke tests and diagnostics.
pub const CRATE_NAME: &str = env!("CARGO_PKG_NAME");
