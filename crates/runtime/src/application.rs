//! Reusable application facade shared by executable frontends.

use std::{path::Path, sync::Arc};

use sonicmux_backend::{BackendCapabilities, CapabilityRequest, MediaBackend, ProgressEvent};
use sonicmux_core::{JobPlan, MediaInfo, PlanOutcome, PlanningPolicy};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::{JobReport, RuntimeError, ValidationMismatch, execute_safely, validate_output};

/// Read-only classification of a planned destination.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExistingOutputOutcome {
    /// No filesystem entry exists.
    Absent,
    /// The existing regular output satisfies every postcondition.
    Valid,
    /// The existing entry is unsafe or does not match the plan.
    Conflict {
        /// Stable bounded postcondition mismatches.
        mismatches: Vec<ValidationMismatch>,
    },
}

/// Backend-neutral application operations for one SonicMux process.
#[derive(Clone)]
pub struct Runtime {
    backend: Arc<dyn MediaBackend>,
}

impl Runtime {
    /// Creates a facade around one configured backend.
    #[must_use]
    pub fn new(backend: Arc<dyn MediaBackend>) -> Self {
        Self { backend }
    }

    /// Probes one local media path.
    pub async fn probe(
        &self,
        input: &Path,
        cancel: CancellationToken,
    ) -> Result<MediaInfo, RuntimeError> {
        self.backend.probe(input, cancel).await.map_err(Into::into)
    }

    /// Builds a pure plan from already-probed media.
    pub fn plan(
        &self,
        media: &MediaInfo,
        policy: &PlanningPolicy,
    ) -> Result<PlanOutcome, RuntimeError> {
        sonicmux_core::build(media, policy).map_err(Into::into)
    }

    /// Inspects an existing final path without mutating it.
    pub async fn inspect_existing_output(
        &self,
        plan: &JobPlan,
        cancel: CancellationToken,
    ) -> Result<ExistingOutputOutcome, RuntimeError> {
        let metadata = match tokio::fs::symlink_metadata(plan.output()).await {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(ExistingOutputOutcome::Absent);
            }
            Err(source) => return Err(RuntimeError::InspectOutput { source }),
        };
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Ok(ExistingOutputOutcome::Conflict {
                mismatches: Vec::new(),
            });
        }
        let actual = self.backend.probe(plan.output(), cancel).await?;
        match validate_output(plan, &actual) {
            Ok(_) => Ok(ExistingOutputOutcome::Valid),
            Err(error) => Ok(ExistingOutputOutcome::Conflict {
                mismatches: error
                    .validation_mismatches()
                    .map_or_else(Vec::new, <[ValidationMismatch]>::to_vec),
            }),
        }
    }

    /// Executes one plan through the safe M3 transaction.
    pub async fn execute(
        &self,
        plan: Arc<JobPlan>,
        progress: mpsc::Sender<ProgressEvent>,
        cancel: CancellationToken,
    ) -> Result<JobReport, RuntimeError> {
        execute_safely(self.backend.as_ref(), plan, progress, cancel)
            .await
            .map_err(Into::into)
    }

    /// Inspects backend tools and requested features.
    pub async fn doctor(
        &self,
        request: CapabilityRequest,
        cancel: CancellationToken,
    ) -> Result<BackendCapabilities, RuntimeError> {
        self.backend
            .capabilities(request, cancel)
            .await
            .map_err(Into::into)
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests {
    use std::{
        path::{Path, PathBuf},
        sync::Arc,
    };

    use async_trait::async_trait;
    use sonicmux_backend::{
        BackendError, BackendExecution, BackendReport, MediaBackend, ProgressEvent,
    };
    use sonicmux_core::MediaInfo;
    use tokio::sync::mpsc;
    use tokio_util::sync::CancellationToken;

    use super::Runtime;

    #[derive(Clone)]
    struct MockBackend {
        media: MediaInfo,
    }

    #[async_trait]
    impl MediaBackend for MockBackend {
        async fn probe(
            &self,
            _path: &Path,
            cancel: CancellationToken,
        ) -> Result<MediaInfo, BackendError> {
            if cancel.is_cancelled() {
                Err(BackendError::Cancelled)
            } else {
                Ok(self.media.clone())
            }
        }

        async fn execute(
            &self,
            _request: BackendExecution,
            _progress: mpsc::Sender<ProgressEvent>,
            _cancel: CancellationToken,
        ) -> Result<BackendReport, BackendError> {
            Err(BackendError::Execute {
                source: Box::new(std::io::Error::other("not used by this test")),
            })
        }
    }

    fn runtime() -> Runtime {
        let media = sonicmux_ffmpeg::parse_probe_output(
            PathBuf::from("movie.mkv"),
            include_bytes!("../../ffmpeg/tests/fixtures/mixed.json"),
        )
        .expect("checked-in fixture parses");
        Runtime::new(Arc::new(MockBackend { media }))
    }

    #[tokio::test]
    async fn probe_delegates_and_cancellation_stays_typed() {
        let runtime = runtime();
        let media = runtime
            .probe(Path::new("movie.mkv"), CancellationToken::new())
            .await
            .expect("mock probe succeeds");
        assert_eq!(media.path(), Path::new("movie.mkv"));

        let cancel = CancellationToken::new();
        cancel.cancel();
        let error = runtime.probe(Path::new("movie.mkv"), cancel).await;
        assert!(matches!(
            error,
            Err(crate::RuntimeError::Backend(BackendError::Cancelled))
        ));
    }
}
