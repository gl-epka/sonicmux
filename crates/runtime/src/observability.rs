//! Shared tracing initialization for executable frontends.

use std::env;

use thiserror::Error;
use tracing_subscriber::EnvFilter;

const DEFAULT_FILTER: &str = "info";

/// An error encountered while configuring the global tracing subscriber.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ObservabilityError {
    /// `RUST_LOG` contains an invalid tracing filter.
    #[error("RUST_LOG is not a valid tracing filter: {reason}")]
    InvalidFilter {
        /// Parser diagnostic without the potentially sensitive filter value.
        reason: String,
    },

    /// The process already has a global tracing subscriber or rejected this one.
    #[error("failed to install the tracing subscriber: {reason}")]
    InstallSubscriber {
        /// Subscriber installation diagnostic.
        reason: String,
    },
}

/// Installs a human-readable tracing subscriber controlled by `RUST_LOG`.
///
/// The default filter is `info`. An invalid non-Unicode or syntactically invalid
/// `RUST_LOG` value is reported rather than silently ignored.
///
/// # Errors
///
/// Returns [`ObservabilityError`] when `RUST_LOG` is invalid or another global
/// subscriber has already been installed.
pub fn init_tracing() -> Result<(), ObservabilityError> {
    let filter = match env::var("RUST_LOG") {
        Ok(value) => {
            EnvFilter::try_new(value).map_err(|error| ObservabilityError::InvalidFilter {
                reason: error.to_string(),
            })?
        }
        Err(env::VarError::NotPresent) => EnvFilter::new(DEFAULT_FILTER),
        Err(env::VarError::NotUnicode(_)) => {
            return Err(ObservabilityError::InvalidFilter {
                reason: "the value is not valid Unicode".to_owned(),
            });
        }
    };

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init()
        .map_err(|error| ObservabilityError::InstallSubscriber {
            reason: error.to_string(),
        })
}
