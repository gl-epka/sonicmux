//! Shared tracing initialization for executable frontends.

use std::{env, fs::OpenOptions, path::PathBuf};

use thiserror::Error;
use tracing_subscriber::{
    EnvFilter, Layer as _, layer::SubscriberExt as _, util::SubscriberInitExt as _,
};

const DEFAULT_FILTER: &str = "info";

/// Tracing destinations and filter selected by a frontend.
#[derive(Debug, Clone)]
pub struct ObservabilityOptions {
    /// Tracing filter expression.
    pub filter: String,
    /// Whether to render human diagnostics to stderr.
    pub console: bool,
    /// Optional newline-delimited JSON log file.
    pub file: Option<PathBuf>,
}

impl Default for ObservabilityOptions {
    fn default() -> Self {
        Self {
            filter: env::var("RUST_LOG").unwrap_or_else(|_| DEFAULT_FILTER.to_owned()),
            console: true,
            file: None,
        }
    }
}

/// Keeps the optional non-blocking file writer alive until orderly shutdown.
pub struct ObservabilityGuard {
    _file_guard: Option<tracing_appender::non_blocking::WorkerGuard>,
}

/// An error encountered while configuring the global tracing subscriber.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ObservabilityError {
    /// The filter expression is invalid.
    #[error("diagnostic filter is invalid: {reason}")]
    InvalidFilter {
        /// Parser diagnostic.
        reason: String,
    },
    /// The structured log file could not be opened.
    #[error("failed to open structured log {}: {source}", path.display())]
    OpenLog {
        /// Requested path.
        path: PathBuf,
        /// Operating-system error.
        #[source]
        source: std::io::Error,
    },
    /// The process already has a global tracing subscriber or rejected this one.
    #[error("failed to install the tracing subscriber: {reason}")]
    InstallSubscriber {
        /// Subscriber installation diagnostic.
        reason: String,
    },
}

/// Installs console tracing controlled by `RUST_LOG`.
///
/// # Errors
///
/// Returns [`ObservabilityError`] when configuration is invalid.
pub fn init_tracing() -> Result<(), ObservabilityError> {
    init_tracing_with(ObservabilityOptions::default()).map(|_guard| ())
}

/// Installs human stderr and optional structured-file tracing.
///
/// # Errors
///
/// Returns [`ObservabilityError`] before command execution when setup fails.
pub fn init_tracing_with(
    options: ObservabilityOptions,
) -> Result<ObservabilityGuard, ObservabilityError> {
    let filter =
        EnvFilter::try_new(&options.filter).map_err(|error| ObservabilityError::InvalidFilter {
            reason: error.to_string(),
        })?;
    let console_filter =
        EnvFilter::try_new(&options.filter).map_err(|error| ObservabilityError::InvalidFilter {
            reason: error.to_string(),
        })?;
    let console = options.console.then(|| {
        tracing_subscriber::fmt::layer()
            .with_ansi(false)
            .with_writer(std::io::stderr)
            .with_filter(console_filter)
    });
    let (file_layer, file_guard) = if let Some(path) = options.file {
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|source| ObservabilityError::OpenLog {
                path: path.clone(),
                source,
            })?;
        let (writer, guard) = tracing_appender::non_blocking(file);
        (
            Some(
                tracing_subscriber::fmt::layer()
                    .json()
                    .with_writer(writer)
                    .with_filter(filter),
            ),
            Some(guard),
        )
    } else {
        (None, None)
    };
    tracing_subscriber::registry()
        .with(console)
        .with(file_layer)
        .try_init()
        .map_err(|error| ObservabilityError::InstallSubscriber {
            reason: error.to_string(),
        })?;
    Ok(ObservabilityGuard {
        _file_guard: file_guard,
    })
}
