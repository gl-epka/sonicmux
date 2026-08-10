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

/// One backend feature required by a frontend operation.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
#[non_exhaustive]
pub enum MediaCapability {
    /// Container demuxer.
    Demuxer(String),
    /// Container muxer.
    Muxer(String),
    /// Audio or video decoder.
    Decoder(String),
    /// Audio or video encoder.
    Encoder(String),
}

impl MediaCapability {
    /// Returns the backend spelling of the capability.
    #[must_use]
    pub fn name(&self) -> &str {
        match self {
            Self::Demuxer(name) | Self::Muxer(name) | Self::Decoder(name) | Self::Encoder(name) => {
                name
            }
        }
    }

    /// Returns a stable capability-kind label.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Demuxer(_) => "demuxer",
            Self::Muxer(_) => "muxer",
            Self::Decoder(_) => "decoder",
            Self::Encoder(_) => "encoder",
        }
    }
}

/// Bounded capability query issued by the application layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityRequest {
    required: Vec<MediaCapability>,
}

impl CapabilityRequest {
    /// Creates a query for the listed capabilities.
    #[must_use]
    pub fn new(required: Vec<MediaCapability>) -> Self {
        Self { required }
    }

    /// Returns requested checks in display order.
    #[must_use]
    pub fn required(&self) -> &[MediaCapability] {
        &self.required
    }
}

/// Availability result for one requested feature.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityCheck {
    capability: MediaCapability,
    available: bool,
    detail: Option<String>,
}

impl CapabilityCheck {
    /// Creates one capability result.
    #[must_use]
    pub fn new(capability: MediaCapability, available: bool, detail: Option<String>) -> Self {
        Self {
            capability,
            available,
            detail,
        }
    }

    /// Returns the checked feature.
    #[must_use]
    pub const fn capability(&self) -> &MediaCapability {
        &self.capability
    }

    /// Returns whether it is available.
    #[must_use]
    pub const fn available(&self) -> bool {
        self.available
    }

    /// Returns an optional bounded diagnostic.
    #[must_use]
    pub fn detail(&self) -> Option<&str> {
        self.detail.as_deref()
    }
}

/// Role of one executable in a backend toolchain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendToolRole {
    /// Media execution tool.
    Ffmpeg,
    /// Media inspection tool.
    Ffprobe,
}

/// Resolved executable and its diagnostic version string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendToolInfo {
    role: BackendToolRole,
    path: PathBuf,
    version: Option<String>,
}

impl BackendToolInfo {
    /// Creates executable information.
    #[must_use]
    pub fn new(role: BackendToolRole, path: PathBuf, version: Option<String>) -> Self {
        Self {
            role,
            path,
            version,
        }
    }

    /// Returns the executable role.
    #[must_use]
    pub const fn role(&self) -> BackendToolRole {
        self.role
    }

    /// Returns the executable path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Returns the first bounded version line, when available.
    #[must_use]
    pub fn version(&self) -> Option<&str> {
        self.version.as_deref()
    }
}

/// Backend diagnostic report independent from a concrete process protocol.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendCapabilities {
    backend_name: String,
    tools: Vec<BackendToolInfo>,
    checks: Vec<CapabilityCheck>,
    warnings: Vec<String>,
}

impl BackendCapabilities {
    /// Creates a complete diagnostic report.
    #[must_use]
    pub fn new(
        backend_name: impl Into<String>,
        tools: Vec<BackendToolInfo>,
        checks: Vec<CapabilityCheck>,
        warnings: Vec<String>,
    ) -> Self {
        Self {
            backend_name: backend_name.into(),
            tools,
            checks,
            warnings,
        }
    }

    /// Returns the adapter name.
    #[must_use]
    pub fn backend_name(&self) -> &str {
        &self.backend_name
    }

    /// Returns tool diagnostics.
    #[must_use]
    pub fn tools(&self) -> &[BackendToolInfo] {
        &self.tools
    }

    /// Returns requested checks.
    #[must_use]
    pub fn checks(&self) -> &[CapabilityCheck] {
        &self.checks
    }

    /// Returns non-fatal warnings.
    #[must_use]
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    /// Returns whether every requested capability is present.
    #[must_use]
    pub fn all_available(&self) -> bool {
        self.checks.iter().all(CapabilityCheck::available)
    }
}

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
    /// Backend capability inspection failed.
    #[error("backend capability inspection failed: {source}")]
    Capability {
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

    /// Inspects required backend capabilities.
    async fn capabilities(
        &self,
        _request: CapabilityRequest,
        _cancel: CancellationToken,
    ) -> Result<BackendCapabilities, BackendError> {
        Err(BackendError::Capability {
            source: Box::new(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "backend does not implement capability inspection",
            )),
        })
    }
}

/// The package name, exposed for workspace diagnostics.
pub const CRATE_NAME: &str = env!("CARGO_PKG_NAME");
