//! Domain error types.

use thiserror::Error;

/// An error produced while validating domain data or building a job plan.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum CoreError {
    /// A configuration value violates a domain invariant.
    #[error("invalid configuration: {message}")]
    InvalidConfiguration {
        /// A user-facing description of the violated invariant.
        message: String,
    },
}

#[cfg(test)]
mod tests {
    use super::CoreError;

    #[test]
    fn error_has_actionable_display_text() {
        let error = CoreError::InvalidConfiguration {
            message: "jobs must be greater than zero".to_owned(),
        };

        assert_eq!(
            error.to_string(),
            "invalid configuration: jobs must be greater than zero"
        );
    }
}
