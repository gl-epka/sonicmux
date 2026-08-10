#![doc = "Pure domain types and planning rules for SonicMux."]
#![forbid(unsafe_code)]

pub mod error;
pub mod model;
pub mod plan;
pub mod policy;

pub use error::CoreError;
pub use model::*;
pub use plan::*;
pub use policy::*;

/// The package name, exposed for workspace smoke tests and diagnostics.
pub const CRATE_NAME: &str = env!("CARGO_PKG_NAME");
