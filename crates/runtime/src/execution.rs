//! Safe single-file execution transaction and post-write validation.

use std::{
    collections::BTreeSet,
    env, io,
    path::{Path, PathBuf},
    sync::Arc,
};

use renamore::rename_exclusive;
use sonicmux_backend::{
    BackendError, BackendExecution, BackendReport, MediaBackend, ProgressEvent,
};
use sonicmux_core::{
    Chapter, ExpectedCodec, ExpectedStream, ExpectedStreamKind, JobPlan, MediaInfo, MediaTimestamp,
    Metadata, OutputStreamPlan, StreamInfo, TimeBase,
};
use tempfile::{Builder, TempDir, TempPath};
use thiserror::Error;
use tokio::{fs, sync::mpsc, task::JoinError};
use tokio_util::sync::CancellationToken;

/// One bounded output-validation mismatch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationMismatch {
    location: String,
    expected: String,
    actual: String,
}

impl ValidationMismatch {
    fn new(
        location: impl Into<String>,
        expected: impl Into<String>,
        actual: impl Into<String>,
    ) -> Self {
        Self {
            location: location.into(),
            expected: bounded(expected.into()),
            actual: bounded(actual.into()),
        }
    }

    /// Returns the stable validation location.
    #[must_use]
    pub fn location(&self) -> &str {
        &self.location
    }

    /// Returns the bounded expected value.
    #[must_use]
    pub fn expected(&self) -> &str {
        &self.expected
    }

    /// Returns the bounded actual value.
    #[must_use]
    pub fn actual(&self) -> &str {
        &self.actual
    }
}

/// Successful structural validation report.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ValidationReport {
    warnings: Vec<String>,
}

impl ValidationReport {
    /// Returns bounded non-fatal validation observations.
    #[must_use]
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }
}

/// Successful safe execution report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobReport {
    output: PathBuf,
    backend: BackendReport,
    validation: ValidationReport,
    warnings: Vec<String>,
}

impl JobReport {
    /// Returns the committed final output path.
    #[must_use]
    pub fn output(&self) -> &Path {
        &self.output
    }

    /// Returns external-process execution facts.
    #[must_use]
    pub const fn backend(&self) -> &BackendReport {
        &self.backend
    }

    /// Returns structural validation facts.
    #[must_use]
    pub const fn validation(&self) -> &ValidationReport {
        &self.validation
    }

    /// Returns transaction warnings, including post-commit cleanup warnings.
    #[must_use]
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }
}

/// Failure of the safe output transaction.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ExecutionError {
    /// Cancellation was observed before or during execution.
    #[error("media execution cancelled")]
    Cancelled,
    /// The output path could not be resolved.
    #[error("failed to resolve output path: {source}")]
    ResolveOutput {
        /// Operating-system error.
        #[source]
        source: io::Error,
    },
    /// The requested final path already exists.
    #[error("output already exists: {path}", path = path.display())]
    OutputExists {
        /// Existing path.
        path: PathBuf,
    },
    /// The output parent is missing or is not a directory.
    #[error("output parent is not an existing directory: {path}", path = path.display())]
    InvalidOutputParent {
        /// Rejected parent.
        path: PathBuf,
    },
    /// Filesystem preflight failed.
    #[error("failed to inspect output path: {source}")]
    Preflight {
        /// Operating-system error.
        #[source]
        source: io::Error,
    },
    /// Private staging creation failed.
    #[error("failed to create private staging output: {source}")]
    CreateStaging {
        /// Operating-system error.
        #[source]
        source: io::Error,
    },
    /// The configured media backend failed.
    #[error(transparent)]
    Backend(#[from] BackendError),
    /// FFmpeg did not leave a usable regular staging file.
    #[error("FFmpeg staging output is not a non-empty regular file")]
    InvalidStagingFile,
    /// Output postconditions did not pass.
    #[error("output validation failed with {count} mismatch(es)")]
    Validation {
        /// Structured bounded mismatches.
        mismatches: Vec<ValidationMismatch>,
        /// Cached count for a concise display message.
        count: usize,
    },
    /// Synchronizing completed media content failed.
    #[error("failed to synchronize staging output: {source}")]
    Sync {
        /// Operating-system error.
        #[source]
        source: io::Error,
    },
    /// A competing writer created the final destination.
    #[error("output appeared before atomic commit: {path}", path = path.display())]
    CommitCollision {
        /// Existing destination.
        path: PathBuf,
    },
    /// Atomic non-replacing rename is unavailable.
    #[error("filesystem does not support atomic non-replacing rename for {path}", path = path.display())]
    AtomicCommitUnsupported {
        /// Requested destination.
        path: PathBuf,
    },
    /// Atomic publication failed.
    #[error("failed to atomically publish output: {source}")]
    Commit {
        /// Operating-system error.
        #[source]
        source: io::Error,
    },
    /// A blocking filesystem task failed unexpectedly.
    #[error("filesystem task failed: {message}")]
    BlockingTask {
        /// Bounded join diagnostic.
        message: String,
    },
    /// Cleanup failed in addition to the primary failure.
    #[error("{primary}; staging cleanup also failed: {cleanup}")]
    Cleanup {
        /// Primary operation failure.
        primary: Box<ExecutionError>,
        /// Cleanup failure.
        #[source]
        cleanup: io::Error,
    },
}

impl ExecutionError {
    /// Returns validation mismatches when this is a validation failure.
    #[must_use]
    pub fn validation_mismatches(&self) -> Option<&[ValidationMismatch]> {
        match self {
            Self::Validation { mismatches, .. } => Some(mismatches),
            _ => None,
        }
    }
}

struct StagingGuard {
    path: TempPath,
    directory: TempDir,
}

impl StagingGuard {
    fn create(parent: &Path) -> io::Result<Self> {
        let directory = Builder::new().prefix(".sonicmux-").tempdir_in(parent)?;
        let file = Builder::new()
            .suffix(".tmp.mkv")
            .tempfile_in(directory.path())?;
        Ok(Self {
            path: file.into_temp_path(),
            directory,
        })
    }

    fn path(&self) -> &Path {
        self.path.as_ref()
    }

    fn cleanup(self) -> io::Result<()> {
        let path_result = match self.path.close() {
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            result => result,
        };
        let directory_result = self.directory.close();
        path_result.and(directory_result)
    }

    fn finish_commit(mut self) -> io::Result<()> {
        self.path.disable_cleanup(true);
        self.directory.close()
    }
}

/// Executes one plan through staging, validation, and atomic publication.
///
/// # Errors
///
/// Returns [`ExecutionError`] without exposing a partial final output. Every
/// non-success path attempts explicit staging cleanup.
pub async fn execute_safely(
    backend: &dyn MediaBackend,
    plan: Arc<JobPlan>,
    progress: mpsc::Sender<ProgressEvent>,
    cancel: CancellationToken,
) -> Result<JobReport, ExecutionError> {
    if cancel.is_cancelled() {
        return Err(ExecutionError::Cancelled);
    }
    let final_path = absolute_path(plan.output())?;
    preflight(final_path.clone()).await?;
    let parent = final_path
        .parent()
        .ok_or_else(|| ExecutionError::InvalidOutputParent {
            path: final_path.clone(),
        })?
        .to_path_buf();
    let staging = spawn_blocking(move || StagingGuard::create(&parent))
        .await?
        .map_err(|source| ExecutionError::CreateStaging { source })?;
    let request = BackendExecution::new(Arc::clone(&plan), staging.path().to_path_buf());
    let backend_report = match backend.execute(request, progress, cancel.clone()).await {
        Ok(report) => report,
        Err(BackendError::Cancelled) => {
            return Err(cleanup_or(ExecutionError::Cancelled, staging).await);
        }
        Err(error) => {
            return Err(cleanup_or(ExecutionError::Backend(error), staging).await);
        }
    };

    if let Err(error) = validate_staging_file(staging.path()).await {
        return Err(cleanup_or(error, staging).await);
    }
    let media = match backend.probe(staging.path(), cancel.clone()).await {
        Ok(media) => media,
        Err(BackendError::Cancelled) => {
            return Err(cleanup_or(ExecutionError::Cancelled, staging).await);
        }
        Err(error) => {
            return Err(cleanup_or(ExecutionError::Backend(error), staging).await);
        }
    };
    let validation = match validate_output(&plan, &media) {
        Ok(report) => report,
        Err(error) => return Err(cleanup_or(error, staging).await),
    };
    if let Err(source) = sync_file(staging.path()).await {
        return Err(cleanup_or(ExecutionError::Sync { source }, staging).await);
    }

    let commit_path = final_path.clone();
    let (staging, commit_result) = spawn_blocking(move || {
        let result = rename_exclusive(staging.path(), &commit_path);
        (staging, result)
    })
    .await?;
    if let Err(source) = commit_result {
        let primary = match source.kind() {
            io::ErrorKind::AlreadyExists => ExecutionError::CommitCollision {
                path: final_path.clone(),
            },
            io::ErrorKind::Unsupported => ExecutionError::AtomicCommitUnsupported {
                path: final_path.clone(),
            },
            _ => ExecutionError::Commit { source },
        };
        return Err(cleanup_or(primary, staging).await);
    }

    let mut warnings = Vec::new();
    if let Err(error) = spawn_blocking(move || staging.finish_commit()).await? {
        warnings.push(format!(
            "committed output but could not remove staging directory: {error}"
        ));
    }
    Ok(JobReport {
        output: final_path,
        backend: backend_report,
        validation,
        warnings,
    })
}

fn absolute_path(path: &Path) -> Result<PathBuf, ExecutionError> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        env::current_dir()
            .map(|directory| directory.join(path))
            .map_err(|source| ExecutionError::ResolveOutput { source })
    }
}

async fn preflight(final_path: PathBuf) -> Result<(), ExecutionError> {
    spawn_blocking(move || {
        match std::fs::symlink_metadata(&final_path) {
            Ok(_) => {
                return Err(ExecutionError::OutputExists {
                    path: final_path.clone(),
                });
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(source) => return Err(ExecutionError::Preflight { source }),
        }
        let Some(parent) = final_path.parent() else {
            return Err(ExecutionError::InvalidOutputParent { path: final_path });
        };
        let metadata =
            std::fs::metadata(parent).map_err(|source| ExecutionError::Preflight { source })?;
        if !metadata.is_dir() {
            return Err(ExecutionError::InvalidOutputParent {
                path: parent.to_path_buf(),
            });
        }
        Ok(())
    })
    .await?
}

async fn validate_staging_file(path: &Path) -> Result<(), ExecutionError> {
    let metadata = fs::symlink_metadata(path)
        .await
        .map_err(|_| ExecutionError::InvalidStagingFile)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() == 0 {
        return Err(ExecutionError::InvalidStagingFile);
    }
    Ok(())
}

async fn sync_file(path: &Path) -> io::Result<()> {
    fs::OpenOptions::new()
        .write(true)
        .open(path)
        .await?
        .sync_all()
        .await
}

async fn cleanup_or(primary: ExecutionError, staging: StagingGuard) -> ExecutionError {
    match spawn_blocking(move || staging.cleanup()).await {
        Ok(Ok(())) => primary,
        Ok(Err(cleanup)) => ExecutionError::Cleanup {
            primary: Box::new(primary),
            cleanup,
        },
        Err(error) => ExecutionError::Cleanup {
            primary: Box::new(primary),
            cleanup: io::Error::other(format!("cleanup task failed: {error}")),
        },
    }
}

async fn spawn_blocking<F, T>(operation: F) -> Result<T, ExecutionError>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(operation)
        .await
        .map_err(blocking_task_error)
}

fn blocking_task_error(error: JoinError) -> ExecutionError {
    ExecutionError::BlockingTask {
        message: error.to_string(),
    }
}

/// Validates probed output against the plan's structural postconditions.
///
/// # Errors
///
/// Returns all bounded mismatches in deterministic order.
pub fn validate_output(
    plan: &JobPlan,
    actual: &MediaInfo,
) -> Result<ValidationReport, ExecutionError> {
    let expected = plan.expected();
    let mut mismatches = Vec::new();
    if !actual.format().is_matroska() {
        mismatches.push(ValidationMismatch::new(
            "format.container",
            "matroska",
            actual.format().names().join(","),
        ));
    }
    if expected.streams().len() != actual.streams().len() {
        mismatches.push(ValidationMismatch::new(
            "streams.count",
            expected.streams().len().to_string(),
            actual.streams().len().to_string(),
        ));
    }
    for (position, ((expected_stream, operation), actual_stream)) in expected
        .streams()
        .iter()
        .zip(plan.streams())
        .zip(actual.streams())
        .enumerate()
    {
        validate_stream(
            position,
            expected_stream,
            operation,
            actual_stream,
            &mut mismatches,
        );
    }
    validate_metadata(
        "format.metadata",
        expected.global_metadata(),
        actual.format().metadata(),
        &mut mismatches,
    );
    validate_chapters(expected.chapters(), actual.chapters(), &mut mismatches);
    if mismatches.is_empty() {
        Ok(ValidationReport::default())
    } else {
        let count = mismatches.len();
        Err(ExecutionError::Validation { mismatches, count })
    }
}

fn validate_stream(
    position: usize,
    expected: &ExpectedStream,
    operation: &OutputStreamPlan,
    actual: &StreamInfo,
    mismatches: &mut Vec<ValidationMismatch>,
) {
    let prefix = format!("streams[{position}]");
    let actual_kind = stream_kind(actual);
    if expected.kind() != &actual_kind {
        mismatches.push(ValidationMismatch::new(
            format!("{prefix}.kind"),
            format!("{:?}", expected.kind()),
            format!("{actual_kind:?}"),
        ));
    }
    let codec_matches = match expected.codec() {
        ExpectedCodec::Copied(codec) => actual.common().codec_name() == codec,
        ExpectedCodec::Encoded(label) => {
            encoded_codec_name(label).is_some_and(|codec| actual.common().codec_name() == codec)
        }
        _ => false,
    };
    if !codec_matches {
        mismatches.push(ValidationMismatch::new(
            format!("{prefix}.codec"),
            format!("{:?}", expected.codec()),
            actual.common().codec_name(),
        ));
    }
    if let OutputStreamPlan::EncodeAudio {
        output_channels, ..
    } = operation
    {
        let actual_channels = actual.as_audio().map(|audio| audio.channels().count());
        if actual_channels != Some(*output_channels) {
            mismatches.push(ValidationMismatch::new(
                format!("{prefix}.channels"),
                output_channels.get().to_string(),
                actual_channels
                    .map_or_else(|| "<missing>".to_owned(), |value| value.get().to_string()),
            ));
        }
    }
    validate_metadata(
        &format!("{prefix}.metadata"),
        expected.metadata(),
        actual.common().metadata(),
        mismatches,
    );
    let expected_flags = enabled_dispositions(expected.dispositions());
    let actual_flags = enabled_dispositions(actual.common().dispositions());
    if expected_flags != actual_flags {
        mismatches.push(ValidationMismatch::new(
            format!("{prefix}.dispositions"),
            format!("{expected_flags:?}"),
            format!("{actual_flags:?}"),
        ));
    }
    validate_timing(&prefix, expected, actual, mismatches);
}

fn validate_metadata(
    location: &str,
    expected: &Metadata,
    actual: &Metadata,
    mismatches: &mut Vec<ValidationMismatch>,
) {
    for (key, expected_value) in expected.iter() {
        if is_volatile_muxer_metadata(key) {
            continue;
        }
        let actual_value = actual
            .iter()
            .find(|(actual_key, _)| actual_key.eq_ignore_ascii_case(key))
            .map(|(_, value)| value);
        if actual_value != Some(expected_value) {
            mismatches.push(ValidationMismatch::new(
                format!("{location}.{key}"),
                expected_value,
                actual_value.unwrap_or("<missing>"),
            ));
        }
    }
}

fn is_volatile_muxer_metadata(key: &str) -> bool {
    let key = key.to_ascii_uppercase();
    matches!(
        key.as_str(),
        "DURATION" | "ENCODER" | "BPS" | "NUMBER_OF_FRAMES" | "NUMBER_OF_BYTES"
    ) || key.starts_with("_STATISTICS_")
}

fn validate_chapters(
    expected: &[Chapter],
    actual: &[Chapter],
    mismatches: &mut Vec<ValidationMismatch>,
) {
    if expected.len() != actual.len() {
        mismatches.push(ValidationMismatch::new(
            "chapters.count",
            expected.len().to_string(),
            actual.len().to_string(),
        ));
    }
    for (position, (expected, actual)) in expected.iter().zip(actual).enumerate() {
        let prefix = format!("chapters[{position}]");
        if !rational_equal(
            expected.start(),
            expected.time_base(),
            actual.start(),
            actual.time_base(),
        ) || !rational_equal(
            expected.end(),
            expected.time_base(),
            actual.end(),
            actual.time_base(),
        ) {
            mismatches.push(ValidationMismatch::new(
                format!("{prefix}.range"),
                format!("{}..{}", expected.start(), expected.end()),
                format!("{}..{}", actual.start(), actual.end()),
            ));
        }
        validate_metadata(
            &format!("{prefix}.metadata"),
            expected.metadata(),
            actual.metadata(),
            mismatches,
        );
    }
}

fn validate_timing(
    prefix: &str,
    expected: &ExpectedStream,
    actual: &StreamInfo,
    mismatches: &mut Vec<ValidationMismatch>,
) {
    let expected_timing = expected.timing();
    let actual_timing = actual.common().timing();
    if let Some(expected_start) = expected_timing.start() {
        let close = actual_timing.start().is_some_and(|actual_start| {
            if let ExpectedCodec::Encoded(label) = expected.codec() {
                actual
                    .as_audio()
                    .and_then(|audio| audio.sample_rate())
                    .is_some_and(|rate| {
                        within_samples(
                            expected_start,
                            actual_start,
                            rate.get(),
                            codec_frame_samples(label),
                        )
                    })
            } else {
                within_actual_ticks(expected_start, actual_start, 1)
            }
        });
        if !close {
            mismatches.push(ValidationMismatch::new(
                format!("{prefix}.start"),
                format!("{:?}", expected_timing.start()),
                format!("{:?}", actual_timing.start()),
            ));
        }
    }
    if let (
        Some(expected_duration),
        Some(actual_duration),
        Some(expected_start),
        Some(actual_start),
    ) = (
        expected_timing.duration_ticks(),
        actual_timing.duration_ticks(),
        expected_timing.start(),
        actual_timing.start(),
    ) {
        let close = match expected.codec() {
            ExpectedCodec::Encoded(label) => actual
                .as_audio()
                .and_then(|audio| audio.sample_rate())
                .is_some_and(|rate| {
                    within_duration_samples(
                        expected_duration,
                        expected_start.time_base(),
                        actual_duration,
                        actual_start.time_base(),
                        rate.get(),
                        codec_frame_samples(label),
                    )
                }),
            _ => within_duration_ticks(
                expected_duration,
                expected_start.time_base(),
                actual_duration,
                actual_start.time_base(),
                1,
            ),
        };
        if !close {
            mismatches.push(ValidationMismatch::new(
                format!("{prefix}.duration"),
                expected_duration.to_string(),
                actual_duration.to_string(),
            ));
        }
    }
}

fn stream_kind(stream: &StreamInfo) -> ExpectedStreamKind {
    match stream {
        StreamInfo::Video(_) => ExpectedStreamKind::Video,
        StreamInfo::Audio(_) => ExpectedStreamKind::Audio,
        StreamInfo::Subtitle(_) => ExpectedStreamKind::Subtitle,
        StreamInfo::Attachment(_) => ExpectedStreamKind::Attachment,
        StreamInfo::Data(_) => ExpectedStreamKind::Data,
        StreamInfo::Unknown(stream) => ExpectedStreamKind::Unknown(stream.kind().to_owned()),
        _ => ExpectedStreamKind::Unknown("future".to_owned()),
    }
}

fn encoded_codec_name(label: &str) -> Option<&'static str> {
    match label {
        "AC-3" => Some("ac3"),
        "E-AC-3" => Some("eac3"),
        "AAC" => Some("aac"),
        _ => None,
    }
}

fn enabled_dispositions(dispositions: &sonicmux_core::Dispositions) -> BTreeSet<String> {
    dispositions
        .to_flags()
        .into_iter()
        .filter_map(|(name, enabled)| enabled.then_some(name))
        .collect()
}

fn rational_equal(left: i64, left_base: TimeBase, right: i64, right_base: TimeBase) -> bool {
    scaled_difference(left, left_base, right, right_base) == 0
}

fn within_actual_ticks(expected: MediaTimestamp, actual: MediaTimestamp, ticks: u32) -> bool {
    let difference = scaled_difference(
        expected.ticks(),
        expected.time_base(),
        actual.ticks(),
        actual.time_base(),
    );
    let tolerance = i128::from(ticks)
        * i128::from(actual.time_base().numerator())
        * i128::from(expected.time_base().denominator());
    difference <= tolerance
}

fn within_samples(
    expected: MediaTimestamp,
    actual: MediaTimestamp,
    sample_rate: u32,
    samples: u32,
) -> bool {
    within_duration_samples(
        expected.ticks(),
        expected.time_base(),
        actual.ticks(),
        actual.time_base(),
        sample_rate,
        samples,
    )
}

fn within_duration_ticks(
    expected: i64,
    expected_base: TimeBase,
    actual: i64,
    actual_base: TimeBase,
    ticks: u32,
) -> bool {
    let difference = scaled_difference(expected, expected_base, actual, actual_base);
    let tolerance = i128::from(ticks)
        * i128::from(actual_base.numerator())
        * i128::from(expected_base.denominator());
    difference <= tolerance
}

fn within_duration_samples(
    expected: i64,
    expected_base: TimeBase,
    actual: i64,
    actual_base: TimeBase,
    sample_rate: u32,
    samples: u32,
) -> bool {
    let difference = scaled_difference(expected, expected_base, actual, actual_base);
    let denominator =
        i128::from(expected_base.denominator()) * i128::from(actual_base.denominator());
    difference * i128::from(sample_rate) <= denominator * i128::from(samples)
}

fn scaled_difference(left: i64, left_base: TimeBase, right: i64, right_base: TimeBase) -> i128 {
    let left =
        i128::from(left) * i128::from(left_base.numerator()) * i128::from(right_base.denominator());
    let right = i128::from(right)
        * i128::from(right_base.numerator())
        * i128::from(left_base.denominator());
    (left - right).abs()
}

fn codec_frame_samples(label: &str) -> u32 {
    match label {
        "AAC" => 1_024,
        _ => 1_536,
    }
}

fn bounded(mut value: String) -> String {
    const MAX_CHARS: usize = 160;
    if value.chars().count() > MAX_CHARS {
        value = value.chars().take(MAX_CHARS).collect();
        value.push('…');
    }
    value
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests {
    use std::{
        collections::BTreeMap,
        path::{Path, PathBuf},
        sync::{
            Arc, Mutex,
            atomic::{AtomicBool, AtomicUsize, Ordering},
        },
        time::Duration,
    };

    use async_trait::async_trait;
    use sonicmux_backend::{
        BackendError, BackendExecution, BackendReport, MediaBackend, ProgressEvent,
    };
    use sonicmux_core::{
        Ac3Bitrate, AudioCodec, AudioStream, AudioTarget, ChannelCount, Channels,
        CompatibilityPolicy, Dispositions, DtsProfile, FormatInfo, MediaInfo, MediaTimestamp,
        Metadata, OutputMode, PlanOutcome, PlanningPolicy, ProfileName, RequestedAction,
        StreamCommon, StreamIndex, StreamInfo, TargetLayout, TimeBase, VideoStream, build,
    };
    use tempfile::TempDir;
    use tokio::sync::mpsc;
    use tokio_util::sync::CancellationToken;

    use super::{ExecutionError, codec_frame_samples, execute_safely, within_samples};

    #[derive(Debug, Clone, Copy)]
    enum Behavior {
        Success,
        Fail,
        Cancel,
        InvalidOutput,
        ProbeFail,
        RaceDestination,
    }

    struct MockBackend {
        output: MediaInfo,
        behavior: Behavior,
        executions: AtomicUsize,
        reaped: AtomicBool,
        staging_path: Mutex<Option<PathBuf>>,
    }

    #[async_trait]
    impl MediaBackend for MockBackend {
        async fn probe(
            &self,
            _path: &Path,
            _cancel: CancellationToken,
        ) -> Result<MediaInfo, BackendError> {
            if matches!(self.behavior, Behavior::ProbeFail) {
                return Err(BackendError::Probe {
                    path: PathBuf::from("<staging>"),
                    source: Box::new(std::io::Error::other("mock probe failure")),
                });
            }
            Ok(self.output.clone())
        }

        async fn execute(
            &self,
            request: BackendExecution,
            _progress: mpsc::Sender<ProgressEvent>,
            _cancel: CancellationToken,
        ) -> Result<BackendReport, BackendError> {
            self.executions.fetch_add(1, Ordering::SeqCst);
            *self.staging_path.lock().map_err(|_| {
                mock_backend_error(std::io::Error::other("mock staging lock poisoned"))
            })? = Some(request.staging_path().to_path_buf());
            tokio::fs::write(request.staging_path(), b"mock matroska")
                .await
                .map_err(mock_backend_error)?;
            let result = match self.behavior {
                Behavior::Success | Behavior::ProbeFail => Ok(BackendReport::new(
                    Duration::from_millis(1),
                    None,
                    Vec::new(),
                )),
                Behavior::Fail => Err(mock_backend_error(std::io::Error::other("mock failure"))),
                Behavior::Cancel => Err(BackendError::Cancelled),
                Behavior::InvalidOutput => {
                    tokio::fs::remove_file(request.staging_path())
                        .await
                        .map_err(mock_backend_error)?;
                    Ok(BackendReport::new(
                        Duration::from_millis(1),
                        None,
                        Vec::new(),
                    ))
                }
                Behavior::RaceDestination => {
                    tokio::fs::write(request.plan().output(), b"competitor")
                        .await
                        .map_err(mock_backend_error)?;
                    Ok(BackendReport::new(
                        Duration::from_millis(1),
                        None,
                        Vec::new(),
                    ))
                }
            };
            self.reaped.store(true, Ordering::SeqCst);
            result
        }
    }

    fn mock_backend(output: MediaInfo, behavior: Behavior) -> MockBackend {
        MockBackend {
            output,
            behavior,
            executions: AtomicUsize::new(0),
            reaped: AtomicBool::new(false),
            staging_path: Mutex::new(None),
        }
    }

    fn mock_backend_error(error: std::io::Error) -> BackendError {
        BackendError::Execute {
            source: Box::new(error),
        }
    }

    fn source_media(path: PathBuf) -> MediaInfo {
        let video = StreamInfo::Video(VideoStream::new(common(0, "hevc")));
        let mut metadata = Metadata::default();
        metadata
            .insert("language", "eng")
            .expect("metadata is valid");
        metadata.insert("title", "Main").expect("metadata is valid");
        let mut flags = BTreeMap::new();
        flags.insert("default".to_owned(), true);
        let audio_common = common(1, "dts")
            .with_metadata(metadata)
            .with_dispositions(Dispositions::from_flags(flags));
        let audio = StreamInfo::Audio(AudioStream::new(
            audio_common,
            AudioCodec::Dts(DtsProfile::Core),
            Channels::new(ChannelCount::new(6).expect("channels are valid"), None),
            None,
        ));
        MediaInfo::new(
            path,
            FormatInfo::new(vec!["matroska".to_owned()]).expect("format is valid"),
            vec![video, audio],
            Vec::new(),
        )
        .expect("media is valid")
    }

    fn output_media(path: PathBuf, valid_codec: bool) -> MediaInfo {
        let video = StreamInfo::Video(VideoStream::new(common(0, "hevc")));
        let mut metadata = Metadata::default();
        metadata
            .insert("language", "eng")
            .expect("metadata is valid");
        metadata
            .insert("title", "Main [AC-3 5.1]")
            .expect("metadata is valid");
        let mut flags = BTreeMap::new();
        flags.insert("default".to_owned(), true);
        let codec_name = if valid_codec { "ac3" } else { "aac" };
        let codec = if valid_codec {
            AudioCodec::Ac3
        } else {
            AudioCodec::Aac
        };
        let audio_common = common(1, codec_name)
            .with_metadata(metadata)
            .with_dispositions(Dispositions::from_flags(flags));
        let audio = StreamInfo::Audio(AudioStream::new(
            audio_common,
            codec,
            Channels::new(ChannelCount::new(6).expect("channels are valid"), None),
            None,
        ));
        MediaInfo::new(
            path,
            FormatInfo::new(vec!["matroska".to_owned()]).expect("format is valid"),
            vec![video, audio],
            Vec::new(),
        )
        .expect("media is valid")
    }

    fn common(index: u32, codec: &str) -> StreamCommon {
        StreamCommon::new(StreamIndex::new(index), codec).expect("codec is valid")
    }

    fn job_plan(directory: &TempDir) -> Arc<sonicmux_core::JobPlan> {
        let input = directory.path().join("movie.mkv");
        let output = directory.path().join("movie.sonicmux.mkv");
        let policy = PlanningPolicy::new(
            CompatibilityPolicy::for_profile(ProfileName::GenericTv),
            AudioTarget::Ac3 {
                bitrate: Ac3Bitrate::new(640_000).expect("bitrate is valid"),
                layout: TargetLayout::KeepUpTo51,
            },
            OutputMode::Replace,
            RequestedAction::Convert,
            output,
        );
        match build(&source_media(input), &policy).expect("plan builds") {
            PlanOutcome::Execute(plan) => Arc::new(plan),
            PlanOutcome::Skip(reason) => panic!("unexpected skip: {reason:?}"),
            _ => panic!("unexpected future plan outcome"),
        }
    }

    async fn run(
        backend: &MockBackend,
        plan: Arc<sonicmux_core::JobPlan>,
    ) -> Result<super::JobReport, ExecutionError> {
        let (sender, _receiver) = mpsc::channel(4);
        execute_safely(backend, plan, sender, CancellationToken::new()).await
    }

    fn assert_no_staging(directory: &TempDir) {
        let staging = std::fs::read_dir(directory.path())
            .expect("directory reads")
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".sonicmux-")
            })
            .count();
        assert_eq!(staging, 0);
    }

    #[test]
    fn encoded_start_accepts_one_codec_frame_of_encoder_priming() {
        let milliseconds = TimeBase::new(1, 1_000).expect("time base is valid");
        let expected = MediaTimestamp::new(0, milliseconds);
        let five_ms_early = MediaTimestamp::new(-5, milliseconds);
        let thirty_three_ms_early = MediaTimestamp::new(-33, milliseconds);
        let frame_samples = codec_frame_samples("AC-3");

        assert!(within_samples(
            expected,
            five_ms_early,
            48_000,
            frame_samples
        ));
        assert!(!within_samples(
            expected,
            thirty_three_ms_early,
            48_000,
            frame_samples
        ));
    }

    #[tokio::test]
    async fn success_validates_and_atomically_publishes() {
        let directory = TempDir::new().expect("temp directory creates");
        let plan = job_plan(&directory);
        let backend = mock_backend(
            output_media(plan.output().to_path_buf(), true),
            Behavior::Success,
        );
        let report = run(&backend, Arc::clone(&plan))
            .await
            .expect("execution succeeds");
        assert_eq!(report.output(), plan.output());
        let staging_path = backend
            .staging_path
            .lock()
            .expect("staging lock is available")
            .clone()
            .expect("backend observed staging path");
        assert_ne!(staging_path, plan.output());
        assert_eq!(
            staging_path.parent().and_then(Path::parent),
            plan.output().parent()
        );
        assert_eq!(
            std::fs::read(plan.output()).expect("output reads"),
            b"mock matroska"
        );
        assert_no_staging(&directory);
    }

    #[tokio::test]
    async fn existing_destination_prevents_backend_execution() {
        let directory = TempDir::new().expect("temp directory creates");
        let plan = job_plan(&directory);
        std::fs::write(plan.output(), b"existing").expect("fixture writes");
        let backend = mock_backend(
            output_media(plan.output().to_path_buf(), true),
            Behavior::Success,
        );
        let error = run(&backend, plan).await.expect_err("collision fails");
        assert!(matches!(error, ExecutionError::OutputExists { .. }));
        assert_eq!(backend.executions.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn competing_destination_is_never_overwritten() {
        let directory = TempDir::new().expect("temp directory creates");
        let plan = job_plan(&directory);
        let backend = mock_backend(
            output_media(plan.output().to_path_buf(), true),
            Behavior::RaceDestination,
        );
        let error = run(&backend, Arc::clone(&plan))
            .await
            .expect_err("commit collision fails");
        assert!(matches!(error, ExecutionError::CommitCollision { .. }));
        assert_eq!(
            std::fs::read(plan.output()).expect("output reads"),
            b"competitor"
        );
        assert_no_staging(&directory);
    }

    #[tokio::test]
    async fn backend_failure_and_cancellation_remove_staging() {
        for behavior in [Behavior::Fail, Behavior::Cancel] {
            let directory = TempDir::new().expect("temp directory creates");
            let plan = job_plan(&directory);
            let backend = mock_backend(output_media(plan.output().to_path_buf(), true), behavior);
            let error = run(&backend, plan).await.expect_err("operation fails");
            assert!(matches!(
                error,
                ExecutionError::Backend(_) | ExecutionError::Cancelled
            ));
            assert!(backend.reaped.load(Ordering::SeqCst));
            assert_no_staging(&directory);
        }
    }

    #[tokio::test]
    async fn invalid_staging_and_failed_probe_remove_staging() {
        for behavior in [Behavior::InvalidOutput, Behavior::ProbeFail] {
            let directory = TempDir::new().expect("temp directory creates");
            let plan = job_plan(&directory);
            let backend = mock_backend(output_media(plan.output().to_path_buf(), true), behavior);
            let error = run(&backend, Arc::clone(&plan))
                .await
                .expect_err("operation fails");
            match behavior {
                Behavior::InvalidOutput => {
                    assert!(matches!(error, ExecutionError::InvalidStagingFile));
                }
                Behavior::ProbeFail => {
                    assert!(matches!(error, ExecutionError::Backend(_)));
                }
                _ => panic!("unexpected test behavior"),
            }
            assert!(backend.reaped.load(Ordering::SeqCst));
            assert!(!plan.output().exists());
            assert_no_staging(&directory);
        }
    }

    #[tokio::test]
    async fn validation_mismatch_removes_staging_and_final() {
        let directory = TempDir::new().expect("temp directory creates");
        let plan = job_plan(&directory);
        let backend = mock_backend(
            output_media(plan.output().to_path_buf(), false),
            Behavior::Success,
        );
        let error = run(&backend, Arc::clone(&plan))
            .await
            .expect_err("validation fails");
        assert!(error.validation_mismatches().is_some());
        assert!(!plan.output().exists());
        assert_no_staging(&directory);
    }
}
