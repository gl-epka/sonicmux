#![doc = "External FFmpeg and FFprobe process adapter for SonicMux."]
#![forbid(unsafe_code)]

pub mod capabilities;
pub mod command;
pub mod discovery;
pub mod error;
pub mod execute;
pub mod probe;
pub mod progress;

pub use command::{ArgumentBuild, build_execution_arguments};
pub use discovery::{
    ResolvedToolchain, ToolchainSource, resolve_toolchain, resolve_toolchain_hybrid,
};
pub use error::ToolError;
pub use execute::ExecutionError;
pub use probe::{FfmpegCliBackend, FfmpegToolchainPaths, ProbeError, parse_probe_output};

/// The package name, exposed for workspace smoke tests and diagnostics.
pub const CRATE_NAME: &str = env!("CARGO_PKG_NAME");

/// Returns the domain crate used by this adapter.
#[must_use]
pub const fn domain_crate_name() -> &'static str {
    sonicmux_core::CRATE_NAME
}
