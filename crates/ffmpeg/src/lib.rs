#![doc = "External FFmpeg and FFprobe process adapter for SonicMux."]
#![forbid(unsafe_code)]

pub mod error;
pub mod probe;

pub use error::BackendError;
pub use probe::{FfmpegCliBackend, ProbeError, parse_probe_output};

/// The package name, exposed for workspace smoke tests and diagnostics.
pub const CRATE_NAME: &str = env!("CARGO_PKG_NAME");

/// Returns the domain crate used by this adapter.
#[must_use]
pub const fn domain_crate_name() -> &'static str {
    sonicmux_core::CRATE_NAME
}
