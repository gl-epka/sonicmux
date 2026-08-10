//! Errors produced by the external media backend.

use std::path::PathBuf;

use thiserror::Error;

/// An error produced while discovering or invoking FFmpeg tools.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ToolError {
    /// An explicit discovery value was neither a directory nor an FFmpeg executable.
    #[error("invalid FFmpeg path: {path}")]
    InvalidPath {
        /// Rejected path.
        path: PathBuf,
    },
    /// A required executable was not found at the resolved location.
    #[error("required executable `{name}` was not found at {path}")]
    ExecutableNotFound {
        /// Executable name used in diagnostics.
        name: &'static str,
        /// Resolved path that could not be executed.
        path: PathBuf,
    },

    /// PATH lookup failed for a required executable.
    #[error("required executable `{name}` was not found on PATH")]
    PathLookup {
        /// Executable name used in diagnostics.
        name: &'static str,
    },

    /// A diagnostic subprocess could not be launched.
    #[error("failed to launch {name} at {path}: {source}")]
    Spawn {
        /// Tool role.
        name: &'static str,
        /// Resolved executable.
        path: PathBuf,
        /// Operating-system error.
        #[source]
        source: std::io::Error,
    },

    /// A diagnostic subprocess exited unsuccessfully.
    #[error("{name} diagnostic command failed: {stderr}")]
    DiagnosticFailed {
        /// Tool role.
        name: &'static str,
        /// Bounded stderr.
        stderr: String,
    },

    /// Reading, waiting for, or terminating a diagnostic subprocess failed.
    #[error("{name} diagnostic {operation} failed: {source}")]
    DiagnosticIo {
        /// Tool role.
        name: &'static str,
        /// Stable operation label.
        operation: &'static str,
        /// Operating-system error.
        #[source]
        source: std::io::Error,
    },

    /// Capability inspection was cancelled.
    #[error("FFmpeg capability inspection cancelled")]
    Cancelled,

    /// The external tool returned data that did not satisfy its protocol.
    #[error("invalid FFmpeg protocol data: {message}")]
    Protocol {
        /// A bounded description of the protocol violation.
        message: String,
    },
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::ToolError;

    #[test]
    fn missing_executable_error_contains_the_path() {
        let error = ToolError::ExecutableNotFound {
            name: "ffmpeg",
            path: PathBuf::from("/missing/ffmpeg"),
        };

        assert!(error.to_string().contains("/missing/ffmpeg"));
    }
}
