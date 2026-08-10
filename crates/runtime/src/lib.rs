#![doc = "Application runtime, scheduling, and observability for SonicMux."]
#![forbid(unsafe_code)]

pub mod error;
pub mod observability;

pub use error::RuntimeError;

/// The package name, exposed for workspace smoke tests and diagnostics.
pub const CRATE_NAME: &str = env!("CARGO_PKG_NAME");

/// Returns the package names that form the non-UI application layers.
#[must_use]
pub const fn application_layers() -> [&'static str; 3] {
    [
        sonicmux_core::CRATE_NAME,
        sonicmux_ffmpeg::CRATE_NAME,
        CRATE_NAME,
    ]
}

#[cfg(test)]
mod tests {
    use super::application_layers;

    #[test]
    fn dependency_layers_are_wired_in_order() {
        assert_eq!(
            application_layers(),
            ["sonicmux-core", "sonicmux-ffmpeg", "sonicmux-runtime"]
        );
    }
}
