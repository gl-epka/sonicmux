#![doc = "Async media backend port and transport-neutral execution events."]
#![forbid(unsafe_code)]

use std::{
    error::Error,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use async_trait::async_trait;
use sonicmux_core::{JobPlan, MediaInfo};
use thiserror::Error;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// A thread-safe error source retained across the backend port.
pub type BoxError = Box<dyn Error + Send + Sync + 'static>;

/// An owned backend execution request.
#[derive(Debug, Clone)]
pub struct BackendExecution {
    plan: Arc<JobPlan>,
    staging_path: PathBuf,
}

impl BackendExecution {
    /// Creates a request for one plan and runtime-owned staging path.
    #[must_use]
    pub fn new(plan: Arc<JobPlan>, staging_path: PathBuf) -> Self {
        Self { plan, staging_path }
    }

    /// Returns the immutable domain plan.
    #[must_use]
    pub fn plan(&self) -> &JobPlan {
        &self.plan
    }

    /// Returns the path the backend is allowed to write.
    #[must_use]
    pub fn staging_path(&self) -> &Path {
        &self.staging_path
    }
}

/// One complete FFmpeg progress record.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProgressSnapshot {
    /// Output timestamp in microseconds, including a possible negative start.
    pub out_time_us: Option<i64>,
    /// Bytes written so far.
    pub total_size_bytes: Option<u64>,
    /// Fixed-point speed where 1,000 means 1.0x.
    pub speed_milli: Option<u32>,
    /// Processed video frames when FFmpeg reports them.
    pub frame: Option<u64>,
    /// Dropped video frames when FFmpeg reports them.
    pub dropped_frames: Option<u64>,
}

/// Best-effort backend progress event.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ProgressEvent {
    /// The external operation has started.
    Started,
    /// A complete intermediate progress record.
    Advanced(ProgressSnapshot),
    /// FFmpeg emitted its terminal progress record.
    Finished(ProgressSnapshot),
}

/// Successful external execution facts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendReport {
    elapsed: Duration,
    last_progress: Option<ProgressSnapshot>,
    warnings: Vec<String>,
}

impl BackendReport {
    /// Creates a successful backend report.
    #[must_use]
    pub fn new(
        elapsed: Duration,
        last_progress: Option<ProgressSnapshot>,
        warnings: Vec<String>,
    ) -> Self {
        Self {
            elapsed,
            last_progress,
            warnings,
        }
    }

    /// Returns elapsed wall-clock time.
    #[must_use]
    pub const fn elapsed(&self) -> Duration {
        self.elapsed
    }

    /// Returns the final complete progress snapshot, when present.
    #[must_use]
    pub const fn last_progress(&self) -> Option<&ProgressSnapshot> {
        self.last_progress.as_ref()
    }

    /// Returns bounded non-fatal backend warnings.
    #[must_use]
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }
}

/// Application-facing backend failure.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum BackendError {
    /// A media probe failed.
    #[error("failed to probe {}: {source}", path.display())]
    Probe {
        /// Probed path.
        path: PathBuf,
        /// Adapter-specific source.
        #[source]
        source: BoxError,
    },
    /// An execution failed.
    #[error("media execution failed: {source}")]
    Execute {
        /// Adapter-specific source.
        #[source]
        source: BoxError,
    },
    /// The operation was cancelled and its process was reaped.
    #[error("media operation cancelled")]
    Cancelled,
}

/// Async application port implemented by media adapters.
#[async_trait]
pub trait MediaBackend: Send + Sync {
    /// Probes one local media path.
    async fn probe(
        &self,
        path: &Path,
        cancel: CancellationToken,
    ) -> Result<MediaInfo, BackendError>;

    /// Executes one plan into its runtime-owned staging path.
    async fn execute(
        &self,
        request: BackendExecution,
        progress: mpsc::Sender<ProgressEvent>,
        cancel: CancellationToken,
    ) -> Result<BackendReport, BackendError>;
}

/// The package name, exposed for workspace diagnostics.
pub const CRATE_NAME: &str = env!("CARGO_PKG_NAME");
