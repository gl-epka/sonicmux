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

    /// Pure media planning failed.
    #[error(transparent)]
    Plan(#[from] sonicmux_core::PlanError),

    /// The safe output transaction failed.
    #[error(transparent)]
    Execution(#[from] crate::ExecutionError),

    /// Inspecting a possible existing output failed.
    #[error("failed to inspect existing output: {source}")]
    InspectOutput {
        /// Operating-system error.
        #[source]
        source: std::io::Error,
    },
}
