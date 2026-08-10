//! Runtime error types.

use thiserror::Error;

/// An error produced at the application runtime boundary.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum RuntimeError {
    /// A domain invariant was violated.
    #[error(transparent)]
    Core(#[from] sonicmux_core::CoreError),

    /// The configured media backend failed.
    #[error(transparent)]
    Backend(#[from] sonicmux_backend::BackendError),
}
