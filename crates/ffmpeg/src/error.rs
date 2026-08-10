//! Errors produced by the external media backend.

use std::path::PathBuf;

use thiserror::Error;

/// An error produced while discovering or invoking FFmpeg tools.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum BackendError {
    /// A required executable was not found at the resolved location.
    #[error("required executable `{name}` was not found at {path}")]
    ExecutableNotFound {
        /// Executable name used in diagnostics.
        name: &'static str,
        /// Resolved path that could not be executed.
        path: PathBuf,
    },

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

    use super::BackendError;

    #[test]
    fn missing_executable_error_contains_the_path() {
        let error = BackendError::ExecutableNotFound {
            name: "ffmpeg",
            path: PathBuf::from("/missing/ffmpeg"),
        };

        assert!(error.to_string().contains("/missing/ffmpeg"));
    }
}
