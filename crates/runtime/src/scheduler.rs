//! Bounded file-level batch scheduling and progress aggregation.

use std::{
    collections::{HashMap, VecDeque},
    env,
    num::NonZeroUsize,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use sonicmux_backend::{BackendError, ProgressEvent, ProgressSnapshot};
use sonicmux_core::{
    AudioSelector, AudioTarget, Compatibility, CompatibilityPolicy, JobPlan, OutputMode,
    PlanOutcome, PlanningPolicy, RequestedAction, StreamIndex,
};
use thiserror::Error;
use tokio::{
    sync::{Semaphore, broadcast, mpsc, watch},
    task::{JoinHandle, JoinSet},
};
use tokio_util::sync::CancellationToken;

use crate::{ExecutionError, ExistingOutputOutcome, JobReport, Runtime, RuntimeError};

const MAX_CONCURRENCY: usize = 64;
const EVENT_CAPACITY: usize = 256;

/// Stable ordinal assigned in input discovery order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct JobId(usize);

impl JobId {
    /// Returns the zero-based discovery ordinal.
    #[must_use]
    pub const fn get(self) -> usize {
        self.0
    }
}

/// Runtime-level audio selection resolved after probing.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum AudioSelectionRequest {
    /// Select the first compatible audio stream.
    FirstCompatible,
    /// Select an exact compatible stream index.
    StreamIndex(StreamIndex),
    /// Select the unique compatible stream with this language tag.
    Language(String),
}

/// High-level operation requested for one input.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ActionRequest {
    /// Convert every incompatible audio stream.
    Convert,
    /// Copy streams and change only the selected default audio disposition.
    RemuxOnly(AudioSelectionRequest),
}

/// Owned, transport-neutral input to one scheduled file operation.
#[derive(Debug, Clone)]
pub struct FileRequest {
    input: PathBuf,
    output: PathBuf,
    compatibility: Arc<CompatibilityPolicy>,
    target: AudioTarget,
    output_mode: OutputMode,
    action: ActionRequest,
}

impl FileRequest {
    /// Creates one fully resolved file request.
    #[must_use]
    pub fn new(
        input: PathBuf,
        output: PathBuf,
        compatibility: Arc<CompatibilityPolicy>,
        target: AudioTarget,
        output_mode: OutputMode,
        action: ActionRequest,
    ) -> Self {
        Self {
            input,
            output,
            compatibility,
            target,
            output_mode,
            action,
        }
    }

    /// Returns the source path.
    #[must_use]
    pub fn input(&self) -> &Path {
        &self.input
    }

    /// Returns the requested final path.
    #[must_use]
    pub fn output(&self) -> &Path {
        &self.output
    }
}

/// Reaction to an individual file failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailurePolicy {
    /// Continue unrelated files and report every outcome.
    Continue,
    /// Stop admission and cancel active files after the first failure.
    FailFast,
}

impl FailurePolicy {
    /// Returns the stable machine spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Continue => "continue",
            Self::FailFast => "fail-fast",
        }
    }
}

/// Validated controls for one batch run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SchedulerOptions {
    max_concurrency: NonZeroUsize,
    failure_policy: FailurePolicy,
    dry_run: bool,
}

impl SchedulerOptions {
    /// Creates scheduler controls.
    ///
    /// # Errors
    ///
    /// Returns [`SchedulerError::InvalidConcurrency`] above the supported cap.
    pub fn new(
        max_concurrency: NonZeroUsize,
        failure_policy: FailurePolicy,
        dry_run: bool,
    ) -> Result<Self, SchedulerError> {
        if max_concurrency.get() > MAX_CONCURRENCY {
            return Err(SchedulerError::InvalidConcurrency {
                value: max_concurrency.get(),
                maximum: MAX_CONCURRENCY,
            });
        }
        Ok(Self {
            max_concurrency,
            failure_policy,
            dry_run,
        })
    }

    /// Returns the maximum simultaneously active files.
    #[must_use]
    pub const fn max_concurrency(self) -> NonZeroUsize {
        self.max_concurrency
    }

    /// Returns the failure policy.
    #[must_use]
    pub const fn failure_policy(self) -> FailurePolicy {
        self.failure_policy
    }

    /// Returns whether execution is disabled.
    #[must_use]
    pub const fn dry_run(self) -> bool {
        self.dry_run
    }
}

/// Complete input to one scheduler run.
#[derive(Debug, Clone)]
pub struct BatchRequest {
    files: Vec<FileRequest>,
    options: SchedulerOptions,
}

impl BatchRequest {
    /// Creates a batch in deterministic discovery order.
    #[must_use]
    pub fn new(files: Vec<FileRequest>, options: SchedulerOptions) -> Self {
        Self { files, options }
    }

    /// Returns the number of requested files.
    #[must_use]
    pub fn len(&self) -> usize {
        self.files.len()
    }

    /// Returns whether no files were requested.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }
}

/// Stable stage at which one file failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureStage {
    /// FFprobe or media parsing.
    Probe,
    /// Remux selector resolution.
    Selection,
    /// Pure plan construction.
    Plan,
    /// Existing or duplicate output conflict.
    Conflict,
    /// FFmpeg execution, validation, or publication.
    Execute,
    /// Unexpected worker task failure.
    Internal,
}

impl FailureStage {
    /// Returns the stable machine spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Probe => "probe",
            Self::Selection => "selection",
            Self::Plan => "plan",
            Self::Conflict => "conflict",
            Self::Execute => "execute",
            Self::Internal => "internal",
        }
    }

    /// Returns the corresponding single-file CLI exit code.
    #[must_use]
    pub const fn exit_code(self) -> u8 {
        match self {
            Self::Probe => 4,
            Self::Selection | Self::Plan => 5,
            Self::Conflict | Self::Execute | Self::Internal => 6,
        }
    }
}

/// Bounded stable diagnostic for one failed file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileFailure {
    stage: FailureStage,
    message: String,
}

impl FileFailure {
    fn new(stage: FailureStage, message: impl Into<String>) -> Self {
        Self {
            stage,
            message: bounded(message.into()),
        }
    }

    /// Returns the failure stage.
    #[must_use]
    pub const fn stage(&self) -> FailureStage {
        self.stage
    }

    /// Returns the bounded diagnostic.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

/// Reason a file needed no execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipReason {
    /// The source already satisfies the selected policy.
    NothingToDo,
    /// A structurally valid destination already exists.
    ValidExistingOutput,
}

impl SkipReason {
    /// Returns the stable machine spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NothingToDo => "nothing-to-do",
            Self::ValidExistingOutput => "valid-existing-output",
        }
    }
}

/// Origin of a scheduler cancellation outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancellationReason {
    /// The caller cancelled the root token.
    User,
    /// An opt-in fail-fast policy cancelled remaining work.
    FailFast,
}

impl CancellationReason {
    /// Returns the stable machine spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::FailFast => "fail-fast",
        }
    }
}

/// Read-only dry-run facts for one planned file.
#[derive(Debug, Clone)]
pub struct DryRunReport {
    plan: Arc<JobPlan>,
    existing: ExistingOutputOutcome,
}

impl DryRunReport {
    /// Returns the immutable execution plan.
    #[must_use]
    pub fn plan(&self) -> &JobPlan {
        &self.plan
    }

    /// Returns the inspected destination state.
    #[must_use]
    pub const fn existing(&self) -> &ExistingOutputOutcome {
        &self.existing
    }
}

/// Authoritative terminal outcome for one requested file.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum FileOutcome {
    /// Output was safely committed.
    Succeeded(JobReport),
    /// No execution was required.
    Skipped(SkipReason),
    /// Dry-run preparation completed.
    Planned(DryRunReport),
    /// This file failed without stopping an ordinary batch.
    Failed(FileFailure),
    /// This file was cancelled before a terminal media result.
    Cancelled(CancellationReason),
}

/// One ordered file result.
#[derive(Debug, Clone)]
pub struct FileResult {
    id: JobId,
    input: PathBuf,
    outcome: FileOutcome,
}

impl FileResult {
    /// Returns the discovery-order identifier.
    #[must_use]
    pub const fn id(&self) -> JobId {
        self.id
    }

    /// Returns the input path.
    #[must_use]
    pub fn input(&self) -> &Path {
        &self.input
    }

    /// Returns the terminal outcome.
    #[must_use]
    pub const fn outcome(&self) -> &FileOutcome {
        &self.outcome
    }
}

/// Authoritative terminal report for a complete batch.
#[derive(Debug, Clone)]
pub struct BatchReport {
    results: Vec<FileResult>,
    cancellation: Option<CancellationReason>,
}

impl BatchReport {
    /// Returns results in discovery order.
    #[must_use]
    pub fn results(&self) -> &[FileResult] {
        &self.results
    }

    /// Returns the batch-level cancellation origin, when any.
    #[must_use]
    pub const fn cancellation(&self) -> Option<CancellationReason> {
        self.cancellation
    }

    /// Returns the number of file failures.
    #[must_use]
    pub fn failure_count(&self) -> usize {
        self.results
            .iter()
            .filter(|result| matches!(result.outcome, FileOutcome::Failed(_)))
            .count()
    }
}

/// Current scheduler phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatchStage {
    /// Inputs are being probed and planned.
    Preparing,
    /// Ready plans are being executed.
    Executing,
    /// The batch reached a non-cancelled terminal state.
    Finished,
    /// User cancellation completed.
    Cancelled,
}

impl BatchStage {
    /// Returns the stable machine spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Preparing => "preparing",
            Self::Executing => "executing",
            Self::Finished => "finished",
            Self::Cancelled => "cancelled",
        }
    }
}

/// Current lifecycle state of one file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileStatus {
    /// Waiting for preparation admission.
    Queued,
    /// Probe and planning are active.
    Preparing,
    /// Prepared and waiting for execution.
    Ready,
    /// FFmpeg transaction is active.
    Running,
    /// Output was committed.
    Succeeded,
    /// No execution was required.
    Skipped,
    /// Dry-run planning completed.
    Planned,
    /// The file failed.
    Failed,
    /// The file was cancelled.
    Cancelled,
}

impl FileStatus {
    /// Returns the stable machine spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Preparing => "preparing",
            Self::Ready => "ready",
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Skipped => "skipped",
            Self::Planned => "planned",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    /// Returns whether no further work can change this file state.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Succeeded | Self::Skipped | Self::Planned | Self::Failed | Self::Cancelled
        )
    }
}

/// Coalesced progress state for one file.
#[derive(Debug, Clone)]
pub struct FileProgressState {
    id: JobId,
    path: PathBuf,
    status: FileStatus,
    duration_us: Option<u64>,
    position_us: Option<u64>,
    speed_milli: Option<u32>,
    eta: Option<Duration>,
    will_execute: bool,
}

impl FileProgressState {
    /// Returns the job identifier.
    #[must_use]
    pub const fn id(&self) -> JobId {
        self.id
    }

    /// Returns the input path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Returns the lifecycle state.
    #[must_use]
    pub const fn status(&self) -> FileStatus {
        self.status
    }

    /// Returns the known media duration in microseconds.
    #[must_use]
    pub const fn duration_us(&self) -> Option<u64> {
        self.duration_us
    }

    /// Returns the clamped media position in microseconds.
    #[must_use]
    pub const fn position_us(&self) -> Option<u64> {
        self.position_us
    }

    /// Returns FFmpeg's fixed-point speed where 1,000 is 1.0x.
    #[must_use]
    pub const fn speed_milli(&self) -> Option<u32> {
        self.speed_milli
    }

    /// Returns the estimated remaining wall-clock duration.
    #[must_use]
    pub const fn eta(&self) -> Option<Duration> {
        self.eta
    }
}

/// Latest coalesced state suitable for UI recovery after event lag.
#[derive(Debug, Clone)]
pub struct BatchSnapshot {
    stage: BatchStage,
    total: usize,
    prepared: usize,
    active: usize,
    queued: usize,
    completed: usize,
    progress_milli: Option<u16>,
    eta: Option<Duration>,
    files: Vec<FileProgressState>,
}

impl BatchSnapshot {
    /// Returns the current batch phase.
    #[must_use]
    pub const fn stage(&self) -> BatchStage {
        self.stage
    }

    /// Returns the total input count.
    #[must_use]
    pub const fn total(&self) -> usize {
        self.total
    }

    /// Returns the number of completed preparations.
    #[must_use]
    pub const fn prepared(&self) -> usize {
        self.prepared
    }

    /// Returns the number of active files.
    #[must_use]
    pub const fn active(&self) -> usize {
        self.active
    }

    /// Returns the number waiting for admission or execution.
    #[must_use]
    pub const fn queued(&self) -> usize {
        self.queued
    }

    /// Returns the number of terminal file outcomes.
    #[must_use]
    pub const fn completed(&self) -> usize {
        self.completed
    }

    /// Returns duration-weighted progress where 1,000 is complete.
    #[must_use]
    pub const fn progress_milli(&self) -> Option<u16> {
        self.progress_milli
    }

    /// Returns the aggregate estimated remaining duration.
    #[must_use]
    pub const fn eta(&self) -> Option<Duration> {
        self.eta
    }

    /// Returns per-file states in discovery order.
    #[must_use]
    pub fn files(&self) -> &[FileProgressState] {
        &self.files
    }
}

/// Best-effort scheduler lifecycle event.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum BatchEvent {
    /// Scheduler supervision started.
    BatchStarted {
        /// Number of files.
        total: usize,
        /// Resolved concurrency limit.
        concurrency: usize,
        /// Failure policy.
        failure_policy: FailurePolicy,
    },
    /// Preparation phase started.
    PreparationStarted,
    /// One file was admitted for preparation.
    FileStarted {
        /// Stable identifier.
        id: JobId,
        /// Input path.
        path: PathBuf,
    },
    /// One file completed preparation.
    FilePrepared {
        /// Stable identifier.
        id: JobId,
        /// Input path.
        path: PathBuf,
        /// Resulting state.
        status: FileStatus,
    },
    /// Execution phase started.
    ExecutionStarted {
        /// Number of ready plans.
        ready: usize,
    },
    /// One best-effort backend progress record.
    FileProgress {
        /// Stable identifier.
        id: JobId,
        /// Input path.
        path: PathBuf,
        /// Raw backend progress.
        progress: ProgressSnapshot,
    },
    /// One file reached a terminal state.
    FileFinished {
        /// Stable identifier.
        id: JobId,
        /// Input path.
        path: PathBuf,
        /// Terminal state.
        status: FileStatus,
    },
    /// Non-cancelled terminal batch event.
    BatchFinished,
    /// User-cancelled terminal batch event.
    BatchCancelled,
}

/// Scheduler construction or supervision failure.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum SchedulerError {
    /// Requested concurrency exceeds the supported bound.
    #[error("scheduler concurrency {value} exceeds maximum {maximum}")]
    InvalidConcurrency {
        /// Rejected value.
        value: usize,
        /// Supported maximum.
        maximum: usize,
    },
    /// The supervisor task failed unexpectedly.
    #[error("batch supervisor task failed: {message}")]
    Supervisor {
        /// Bounded task diagnostic.
        message: String,
    },
}

/// Awaitable owner of the authoritative supervisor result.
pub struct BatchWaiter {
    task: JoinHandle<BatchReport>,
}

impl BatchWaiter {
    /// Waits for every admitted child to finish cleanup and returns final truth.
    pub async fn wait(self) -> Result<BatchReport, SchedulerError> {
        self.task.await.map_err(|error| SchedulerError::Supervisor {
            message: bounded(error.to_string()),
        })
    }
}

/// Running batch subscriptions and authoritative waiter.
pub struct BatchHandle {
    snapshots: watch::Receiver<Arc<BatchSnapshot>>,
    events: broadcast::Receiver<BatchEvent>,
    waiter: BatchWaiter,
}

impl BatchHandle {
    /// Splits the handle for concurrent rendering and authoritative waiting.
    #[must_use]
    pub fn into_parts(
        self,
    ) -> (
        watch::Receiver<Arc<BatchSnapshot>>,
        broadcast::Receiver<BatchEvent>,
        BatchWaiter,
    ) {
        (self.snapshots, self.events, self.waiter)
    }

    /// Waits without consuming intermediate events.
    pub async fn wait(self) -> Result<BatchReport, SchedulerError> {
        self.waiter.wait().await
    }
}

#[derive(Debug)]
enum PreparedResult {
    Ready(Arc<JobPlan>),
    Terminal(FileOutcome),
}

#[derive(Debug)]
struct PreparedTask {
    index: usize,
    result: PreparedResult,
}

#[derive(Debug)]
struct ExecutionTask {
    index: usize,
    result: Result<JobReport, RuntimeError>,
}

#[derive(Debug)]
struct TaggedProgress {
    index: usize,
    event: ProgressEvent,
}

struct MutableState {
    stage: BatchStage,
    files: Vec<FileProgressState>,
    execution_started: Option<Instant>,
    advancing_samples: usize,
}

impl MutableState {
    fn new(files: &[FileRequest]) -> Self {
        Self {
            stage: BatchStage::Preparing,
            files: files
                .iter()
                .enumerate()
                .map(|(index, request)| FileProgressState {
                    id: JobId(index),
                    path: request.input.clone(),
                    status: FileStatus::Queued,
                    duration_us: None,
                    position_us: None,
                    speed_milli: None,
                    eta: None,
                    will_execute: false,
                })
                .collect(),
            execution_started: None,
            advancing_samples: 0,
        }
    }

    fn snapshot(&self) -> Arc<BatchSnapshot> {
        let total = self.files.len();
        let prepared = self
            .files
            .iter()
            .filter(|file| {
                file.status != FileStatus::Queued && file.status != FileStatus::Preparing
            })
            .count();
        let active = self
            .files
            .iter()
            .filter(|file| matches!(file.status, FileStatus::Preparing | FileStatus::Running))
            .count();
        let queued = self
            .files
            .iter()
            .filter(|file| matches!(file.status, FileStatus::Queued | FileStatus::Ready))
            .count();
        let completed = self
            .files
            .iter()
            .filter(|file| file.status.is_terminal())
            .count();
        let (progress_milli, eta) = self.aggregate_progress();
        Arc::new(BatchSnapshot {
            stage: self.stage,
            total,
            prepared,
            active,
            queued,
            completed,
            progress_milli,
            eta,
            files: self.files.clone(),
        })
    }

    fn aggregate_progress(&self) -> (Option<u16>, Option<Duration>) {
        if self.stage != BatchStage::Executing {
            return (None, None);
        }
        let executable: Vec<_> = self.files.iter().filter(|file| file.will_execute).collect();
        if executable.is_empty()
            || executable
                .iter()
                .any(|file| file.duration_us.is_none_or(|value| value == 0))
        {
            return (None, None);
        }
        let total = executable
            .iter()
            .filter_map(|file| file.duration_us)
            .fold(0_u128, |sum, value| sum.saturating_add(u128::from(value)));
        if total == 0 {
            return (None, None);
        }
        let position = executable.iter().fold(0_u128, |sum, file| {
            let duration = file.duration_us.unwrap_or_default();
            let value = if file.status == FileStatus::Succeeded {
                duration
            } else {
                file.position_us.unwrap_or_default().min(duration)
            };
            sum.saturating_add(u128::from(value))
        });
        let milli = position
            .saturating_mul(1_000)
            .checked_div(total)
            .and_then(|value| u16::try_from(value).ok())
            .map(|value| value.min(1_000));
        let eta = self.execution_started.and_then(|started| {
            let elapsed = started.elapsed();
            if self.advancing_samples < 2 || elapsed < Duration::from_secs(2) || position == 0 {
                return None;
            }
            let remaining = total.saturating_sub(position);
            let elapsed_micros = elapsed.as_micros();
            let eta_micros = remaining
                .saturating_mul(elapsed_micros)
                .checked_div(position)?;
            u64::try_from(eta_micros).ok().map(Duration::from_micros)
        });
        (milli, eta)
    }

    fn update_progress(&mut self, index: usize, progress: &ProgressSnapshot) {
        let Some(file) = self.files.get_mut(index) else {
            return;
        };
        let previous = file.position_us;
        file.speed_milli = progress.speed_milli;
        file.position_us = progress
            .out_time_us
            .and_then(|value| u64::try_from(value).ok())
            .map(|value| {
                file.duration_us
                    .map_or(value, |duration| value.min(duration))
            });
        file.eta = match (file.duration_us, file.position_us, file.speed_milli) {
            (Some(duration), Some(position), Some(speed)) if speed > 0 && position < duration => {
                let remaining = duration - position;
                remaining
                    .checked_mul(1_000)
                    .and_then(|value| value.checked_div(u64::from(speed)))
                    .map(Duration::from_micros)
            }
            _ => None,
        };
        if file.position_us > previous {
            self.advancing_samples = self.advancing_samples.saturating_add(1);
        }
    }
}

impl Runtime {
    /// Starts a bounded batch and returns subscriptions plus an authoritative waiter.
    #[must_use]
    pub fn start_batch(&self, request: BatchRequest, cancel: CancellationToken) -> BatchHandle {
        let initial = MutableState::new(&request.files).snapshot();
        let (snapshot_tx, snapshot_rx) = watch::channel(initial);
        let (event_tx, event_rx) = broadcast::channel(EVENT_CAPACITY);
        let runtime = self.clone();
        let task = tokio::spawn(async move {
            supervise(runtime, request, cancel, snapshot_tx, event_tx).await
        });
        BatchHandle {
            snapshots: snapshot_rx,
            events: event_rx,
            waiter: BatchWaiter { task },
        }
    }
}

async fn supervise(
    runtime: Runtime,
    request: BatchRequest,
    cancel: CancellationToken,
    snapshot_tx: watch::Sender<Arc<BatchSnapshot>>,
    event_tx: broadcast::Sender<BatchEvent>,
) -> BatchReport {
    let total = request.files.len();
    let options = request.options;
    let mut state = MutableState::new(&request.files);
    emit(
        &event_tx,
        BatchEvent::BatchStarted {
            total,
            concurrency: options.max_concurrency.get(),
            failure_policy: options.failure_policy,
        },
    );
    emit(&event_tx, BatchEvent::PreparationStarted);
    publish(&snapshot_tx, &state);

    let mut terminal: Vec<Option<FileOutcome>> =
        std::iter::repeat_with(|| None).take(total).collect();
    let mut prepared: Vec<Option<Arc<JobPlan>>> =
        std::iter::repeat_with(|| None).take(total).collect();
    let user_cancelled = prepare_phase(
        &runtime,
        request.files,
        options,
        &cancel,
        &event_tx,
        &snapshot_tx,
        &mut state,
        &mut terminal,
        &mut prepared,
    )
    .await;

    if !user_cancelled && !options.dry_run {
        reject_duplicate_destinations(
            &mut prepared,
            &mut terminal,
            &mut state,
            &event_tx,
            &snapshot_tx,
        )
        .await;
        if options.failure_policy == FailurePolicy::FailFast
            && terminal
                .iter()
                .any(|outcome| matches!(outcome, Some(FileOutcome::Failed(_))))
        {
            for (index, plan) in prepared.iter_mut().enumerate() {
                if plan.take().is_some() && terminal[index].is_none() {
                    terminal[index] = Some(FileOutcome::Cancelled(CancellationReason::FailFast));
                    state.files[index].status = FileStatus::Cancelled;
                    emit(
                        &event_tx,
                        BatchEvent::FileFinished {
                            id: JobId(index),
                            path: state.files[index].path.clone(),
                            status: FileStatus::Cancelled,
                        },
                    );
                }
            }
            publish(&snapshot_tx, &state);
        }
    }

    let ready = prepared.iter().filter(|value| value.is_some()).count();
    if !user_cancelled && !options.dry_run && ready > 0 {
        state.stage = BatchStage::Executing;
        state.execution_started = Some(Instant::now());
        emit(&event_tx, BatchEvent::ExecutionStarted { ready });
        publish(&snapshot_tx, &state);
        let execution_cancelled = execute_phase(
            &runtime,
            options,
            &cancel,
            &event_tx,
            &snapshot_tx,
            &mut state,
            &mut terminal,
            prepared,
        )
        .await;
        finish_report(
            terminal,
            state,
            event_tx,
            snapshot_tx,
            user_cancelled || execution_cancelled,
        )
    } else {
        finish_report(terminal, state, event_tx, snapshot_tx, user_cancelled)
    }
}

#[allow(clippy::too_many_arguments)]
async fn prepare_phase(
    runtime: &Runtime,
    files: Vec<FileRequest>,
    options: SchedulerOptions,
    root_cancel: &CancellationToken,
    event_tx: &broadcast::Sender<BatchEvent>,
    snapshot_tx: &watch::Sender<Arc<BatchSnapshot>>,
    state: &mut MutableState,
    terminal: &mut [Option<FileOutcome>],
    prepared: &mut [Option<Arc<JobPlan>>],
) -> bool {
    let semaphore = Arc::new(Semaphore::new(options.max_concurrency.get()));
    let phase_cancel = root_cancel.child_token();
    let mut pending: VecDeque<_> = files.into_iter().enumerate().collect();
    let mut tasks = JoinSet::new();
    let mut task_ids = HashMap::new();
    let mut stop_admission = false;
    let mut user_cancelled = root_cancel.is_cancelled();
    let mut fail_fast_triggered = false;

    if user_cancelled {
        stop_admission = true;
        cancel_pending(
            &mut pending,
            terminal,
            state,
            event_tx,
            CancellationReason::User,
        );
        publish(snapshot_tx, state);
    }

    loop {
        if !stop_admission {
            while let Some((index, file)) = pending.pop_front() {
                let Ok(permit) = Arc::clone(&semaphore).try_acquire_owned() else {
                    pending.push_front((index, file));
                    break;
                };
                state.files[index].status = FileStatus::Preparing;
                emit(
                    event_tx,
                    BatchEvent::FileStarted {
                        id: JobId(index),
                        path: state.files[index].path.clone(),
                    },
                );
                publish(snapshot_tx, state);
                let runtime = runtime.clone();
                let cancel = phase_cancel.child_token();
                let abort = tasks.spawn(async move {
                    let _permit = permit;
                    prepare_one(runtime, index, file, options.dry_run, cancel).await
                });
                task_ids.insert(abort.id(), index);
            }
        }

        if tasks.is_empty() {
            break;
        }

        tokio::select! {
            () = root_cancel.cancelled(), if !user_cancelled => {
                user_cancelled = true;
                stop_admission = true;
                phase_cancel.cancel();
                cancel_pending(
                    &mut pending,
                    terminal,
                    state,
                    event_tx,
                    CancellationReason::User,
                );
                publish(snapshot_tx, state);
            }
            joined = tasks.join_next_with_id() => {
                let Some(joined) = joined else { continue; };
                match joined {
                    Ok((task_id, task)) => {
                        task_ids.remove(&task_id);
                        let index = task.index;
                        let status = match task.result {
                            PreparedResult::Ready(plan) => {
                                state.files[index].duration_us = plan.duration().map(|value| value.get());
                                state.files[index].will_execute = true;
                                state.files[index].status = FileStatus::Ready;
                                prepared[index] = Some(plan);
                                FileStatus::Ready
                            }
                            PreparedResult::Terminal(mut outcome) => {
                                if matches!(outcome, FileOutcome::Cancelled(_)) {
                                    outcome = if root_cancel.is_cancelled() {
                                        user_cancelled = true;
                                        stop_admission = true;
                                        phase_cancel.cancel();
                                        cancel_pending(
                                            &mut pending,
                                            terminal,
                                            state,
                                            event_tx,
                                            CancellationReason::User,
                                        );
                                        FileOutcome::Cancelled(CancellationReason::User)
                                    } else if fail_fast_triggered {
                                        FileOutcome::Cancelled(CancellationReason::FailFast)
                                    } else {
                                        FileOutcome::Failed(FileFailure::new(
                                            FailureStage::Internal,
                                            "backend cancelled without a scheduler cancellation",
                                        ))
                                    };
                                }
                                let status = status_for_outcome(&outcome);
                                state.files[index].status = status;
                                let failed = matches!(outcome, FileOutcome::Failed(_));
                                terminal[index] = Some(outcome);
                                if failed && options.failure_policy == FailurePolicy::FailFast {
                                    fail_fast_triggered = true;
                                    stop_admission = true;
                                    phase_cancel.cancel();
                                    cancel_pending(
                                        &mut pending,
                                        terminal,
                                        state,
                                        event_tx,
                                        CancellationReason::FailFast,
                                    );
                                }
                                status
                            }
                        };
                        emit(event_tx, BatchEvent::FilePrepared {
                            id: JobId(index),
                            path: state.files[index].path.clone(),
                            status,
                        });
                        if status.is_terminal() {
                            emit(event_tx, BatchEvent::FileFinished {
                                id: JobId(index),
                                path: state.files[index].path.clone(),
                                status,
                            });
                        }
                    }
                    Err(error) => {
                        let index = task_ids.remove(&error.id());
                        if let Some(index) = index {
                            let outcome = if user_cancelled {
                                FileOutcome::Cancelled(CancellationReason::User)
                            } else if fail_fast_triggered {
                                FileOutcome::Cancelled(CancellationReason::FailFast)
                            } else {
                                FileOutcome::Failed(FileFailure::new(
                                    FailureStage::Internal,
                                    error.to_string(),
                                ))
                            };
                            let status = status_for_outcome(&outcome);
                            terminal[index] = Some(outcome);
                            state.files[index].status = status;
                            emit(event_tx, BatchEvent::FileFinished {
                                id: JobId(index),
                                path: state.files[index].path.clone(),
                                status,
                            });
                            if status == FileStatus::Failed
                                && options.failure_policy == FailurePolicy::FailFast
                            {
                                fail_fast_triggered = true;
                                stop_admission = true;
                                phase_cancel.cancel();
                                cancel_pending(
                                    &mut pending,
                                    terminal,
                                    state,
                                    event_tx,
                                    CancellationReason::FailFast,
                                );
                            }
                        }
                    }
                }
                publish(snapshot_tx, state);
            }
        }
    }

    if user_cancelled || fail_fast_triggered {
        let reason = if user_cancelled {
            CancellationReason::User
        } else {
            CancellationReason::FailFast
        };
        for (index, plan) in prepared.iter_mut().enumerate() {
            if plan.take().is_some() && terminal[index].is_none() {
                terminal[index] = Some(FileOutcome::Cancelled(reason));
                state.files[index].status = FileStatus::Cancelled;
                emit(
                    event_tx,
                    BatchEvent::FileFinished {
                        id: JobId(index),
                        path: state.files[index].path.clone(),
                        status: FileStatus::Cancelled,
                    },
                );
            }
        }
        publish(snapshot_tx, state);
    }
    user_cancelled
}

async fn prepare_one(
    runtime: Runtime,
    index: usize,
    file: FileRequest,
    dry_run: bool,
    cancel: CancellationToken,
) -> PreparedTask {
    let media = match runtime.probe(&file.input, cancel.clone()).await {
        Ok(media) => media,
        Err(error) => {
            return PreparedTask {
                index,
                result: PreparedResult::Terminal(outcome_from_runtime(error, FailureStage::Probe)),
            };
        }
    };
    let action = match resolve_action(&media, &file.compatibility, &file.action) {
        Ok(action) => action,
        Err(failure) => {
            return PreparedTask {
                index,
                result: PreparedResult::Terminal(FileOutcome::Failed(failure)),
            };
        }
    };
    let policy = PlanningPolicy::new(
        (*file.compatibility).clone(),
        file.target,
        file.output_mode,
        action,
        file.output,
    );
    let plan = match runtime.plan(&media, &policy) {
        Ok(PlanOutcome::Execute(plan)) => Arc::new(plan),
        Ok(PlanOutcome::Skip(_)) => {
            return PreparedTask {
                index,
                result: PreparedResult::Terminal(FileOutcome::Skipped(SkipReason::NothingToDo)),
            };
        }
        Ok(_) => {
            return PreparedTask {
                index,
                result: PreparedResult::Terminal(FileOutcome::Failed(FileFailure::new(
                    FailureStage::Plan,
                    "planner returned an unsupported outcome",
                ))),
            };
        }
        Err(error) => {
            return PreparedTask {
                index,
                result: PreparedResult::Terminal(outcome_from_runtime(error, FailureStage::Plan)),
            };
        }
    };
    let existing = match runtime.inspect_existing_output(&plan, cancel.clone()).await {
        Ok(existing) => existing,
        Err(error) => {
            return PreparedTask {
                index,
                result: PreparedResult::Terminal(outcome_from_runtime(
                    error,
                    FailureStage::Conflict,
                )),
            };
        }
    };
    let result = if dry_run {
        PreparedResult::Terminal(FileOutcome::Planned(DryRunReport { plan, existing }))
    } else {
        match existing {
            ExistingOutputOutcome::Absent => PreparedResult::Ready(plan),
            ExistingOutputOutcome::Valid => {
                PreparedResult::Terminal(FileOutcome::Skipped(SkipReason::ValidExistingOutput))
            }
            ExistingOutputOutcome::Conflict { mismatches } => {
                PreparedResult::Terminal(FileOutcome::Failed(FileFailure::new(
                    FailureStage::Conflict,
                    format!(
                        "output already exists and conflicts ({} mismatch(es))",
                        mismatches.len()
                    ),
                )))
            }
        }
    };
    PreparedTask { index, result }
}

fn resolve_action(
    media: &sonicmux_core::MediaInfo,
    policy: &CompatibilityPolicy,
    request: &ActionRequest,
) -> Result<RequestedAction, FileFailure> {
    let ActionRequest::RemuxOnly(request) = request else {
        return Ok(RequestedAction::Convert);
    };
    let selection = match request {
        AudioSelectionRequest::FirstCompatible => AudioSelector::FirstCompatible,
        AudioSelectionRequest::StreamIndex(index) => AudioSelector::StreamIndex(*index),
        AudioSelectionRequest::Language(requested) => {
            let mut candidates = Vec::new();
            for stream in media.audio_streams() {
                let Some(language) = stream.common().metadata().language() else {
                    continue;
                };
                if !language.as_str().eq_ignore_ascii_case(requested) {
                    continue;
                }
                let classification = policy.classify(stream).map_err(|error| {
                    FileFailure::new(FailureStage::Selection, error.to_string())
                })?;
                if matches!(classification, Compatibility::Compatible) {
                    candidates.push(stream.common().index());
                }
            }
            match candidates.as_slice() {
                [index] => AudioSelector::StreamIndex(*index),
                [] => {
                    return Err(FileFailure::new(
                        FailureStage::Selection,
                        format!("no compatible audio stream matches language `{requested}`"),
                    ));
                }
                many => {
                    return Err(FileFailure::new(
                        FailureStage::Selection,
                        format!(
                            "language `{requested}` is ambiguous; matching stream indices: {}",
                            many.iter()
                                .map(ToString::to_string)
                                .collect::<Vec<_>>()
                                .join(", ")
                        ),
                    ));
                }
            }
        }
    };
    Ok(RequestedAction::RemuxOnly { selection })
}

async fn reject_duplicate_destinations(
    prepared: &mut [Option<Arc<JobPlan>>],
    terminal: &mut [Option<FileOutcome>],
    state: &mut MutableState,
    event_tx: &broadcast::Sender<BatchEvent>,
    snapshot_tx: &watch::Sender<Arc<BatchSnapshot>>,
) {
    let mut destinations: HashMap<PathBuf, Vec<usize>> = HashMap::new();
    for (index, plan) in prepared.iter().enumerate() {
        let Some(plan) = plan else { continue };
        match destination_key(plan.output()).await {
            Ok(key) => destinations.entry(key).or_default().push(index),
            Err(failure) => {
                terminal[index] = Some(FileOutcome::Failed(failure));
            }
        }
    }
    for indices in destinations
        .into_values()
        .filter(|indices| indices.len() > 1)
    {
        for index in indices {
            terminal[index] = Some(FileOutcome::Failed(FileFailure::new(
                FailureStage::Conflict,
                "multiple inputs resolve to the same output path",
            )));
        }
    }
    for (index, outcome) in terminal.iter().enumerate() {
        if matches!(outcome, Some(FileOutcome::Failed(_))) && prepared[index].take().is_some() {
            state.files[index].status = FileStatus::Failed;
            emit(
                event_tx,
                BatchEvent::FileFinished {
                    id: JobId(index),
                    path: state.files[index].path.clone(),
                    status: FileStatus::Failed,
                },
            );
        }
    }
    publish(snapshot_tx, state);
}

async fn destination_key(path: &Path) -> Result<PathBuf, FileFailure> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        env::current_dir()
            .map(|directory| directory.join(path))
            .map_err(|error| FileFailure::new(FailureStage::Conflict, error.to_string()))?
    };
    let parent = absolute.parent().ok_or_else(|| {
        FileFailure::new(FailureStage::Conflict, "output has no parent directory")
    })?;
    let name = absolute
        .file_name()
        .ok_or_else(|| FileFailure::new(FailureStage::Conflict, "output has no file name"))?;
    tokio::fs::canonicalize(parent)
        .await
        .map(|resolved| resolved.join(name))
        .map_err(|error| FileFailure::new(FailureStage::Conflict, error.to_string()))
}

#[allow(clippy::too_many_arguments)]
async fn execute_phase(
    runtime: &Runtime,
    options: SchedulerOptions,
    root_cancel: &CancellationToken,
    event_tx: &broadcast::Sender<BatchEvent>,
    snapshot_tx: &watch::Sender<Arc<BatchSnapshot>>,
    state: &mut MutableState,
    terminal: &mut [Option<FileOutcome>],
    prepared: Vec<Option<Arc<JobPlan>>>,
) -> bool {
    let semaphore = Arc::new(Semaphore::new(options.max_concurrency.get()));
    let phase_cancel = root_cancel.child_token();
    let mut pending: VecDeque<_> = prepared
        .into_iter()
        .enumerate()
        .filter_map(|(index, plan)| plan.map(|plan| (index, plan)))
        .collect();
    let progress_capacity = options
        .max_concurrency
        .get()
        .saturating_mul(32)
        .clamp(32, 2_048);
    let (progress_tx, mut progress_rx) = mpsc::channel(progress_capacity);
    let mut tasks = JoinSet::new();
    let mut task_ids = HashMap::new();
    let mut stop_admission = false;
    let mut user_cancelled = root_cancel.is_cancelled();
    let mut fail_fast_triggered = false;

    if user_cancelled {
        stop_admission = true;
        cancel_execution_pending(
            &mut pending,
            terminal,
            state,
            event_tx,
            CancellationReason::User,
        );
        publish(snapshot_tx, state);
    }

    loop {
        if !stop_admission {
            while let Some((index, plan)) = pending.pop_front() {
                let Ok(permit) = Arc::clone(&semaphore).try_acquire_owned() else {
                    pending.push_front((index, plan));
                    break;
                };
                state.files[index].status = FileStatus::Running;
                publish(snapshot_tx, state);
                let runtime = runtime.clone();
                let cancel = phase_cancel.child_token();
                let tagged = progress_tx.clone();
                let abort = tasks.spawn(async move {
                    let _permit = permit;
                    execute_one(runtime, index, plan, tagged, cancel).await
                });
                task_ids.insert(abort.id(), index);
            }
        }

        if tasks.is_empty() {
            break;
        }

        tokio::select! {
            () = root_cancel.cancelled(), if !user_cancelled => {
                user_cancelled = true;
                stop_admission = true;
                phase_cancel.cancel();
                cancel_execution_pending(
                    &mut pending,
                    terminal,
                    state,
                    event_tx,
                    CancellationReason::User,
                );
                publish(snapshot_tx, state);
            }
            progress = progress_rx.recv() => {
                if let Some(progress) = progress {
                    let snapshot = match progress.event {
                        ProgressEvent::Started => None,
                        ProgressEvent::Advanced(value) | ProgressEvent::Finished(value) => Some(value),
                        _ => None,
                    };
                    if let Some(snapshot) = snapshot {
                        state.update_progress(progress.index, &snapshot);
                        emit(event_tx, BatchEvent::FileProgress {
                            id: JobId(progress.index),
                            path: state.files[progress.index].path.clone(),
                            progress: snapshot,
                        });
                        publish(snapshot_tx, state);
                    }
                }
            }
            joined = tasks.join_next_with_id() => {
                let Some(joined) = joined else { continue; };
                let (index, outcome) = match joined {
                    Ok((task_id, task)) => {
                        task_ids.remove(&task_id);
                        let outcome = match task.result {
                            Ok(report) => FileOutcome::Succeeded(report),
                            Err(error) if runtime_error_cancelled(&error) => {
                                if root_cancel.is_cancelled() {
                                    user_cancelled = true;
                                    stop_admission = true;
                                    phase_cancel.cancel();
                                    cancel_execution_pending(
                                        &mut pending,
                                        terminal,
                                        state,
                                        event_tx,
                                        CancellationReason::User,
                                    );
                                    FileOutcome::Cancelled(CancellationReason::User)
                                } else if fail_fast_triggered {
                                    FileOutcome::Cancelled(CancellationReason::FailFast)
                                } else {
                                    FileOutcome::Failed(FileFailure::new(
                                        FailureStage::Internal,
                                        "backend cancelled without a scheduler cancellation",
                                    ))
                                }
                            }
                            Err(error) => FileOutcome::Failed(FileFailure::new(
                                FailureStage::Execute,
                                error.to_string(),
                            )),
                        };
                        (task.index, outcome)
                    }
                    Err(error) => {
                        let Some(index) = task_ids.remove(&error.id()) else {
                            continue;
                        };
                        let outcome = if user_cancelled {
                            FileOutcome::Cancelled(CancellationReason::User)
                        } else if fail_fast_triggered {
                            FileOutcome::Cancelled(CancellationReason::FailFast)
                        } else {
                            FileOutcome::Failed(FileFailure::new(
                                FailureStage::Internal,
                                error.to_string(),
                            ))
                        };
                        (index, outcome)
                    }
                };
                let failed = matches!(outcome, FileOutcome::Failed(_));
                let status = status_for_outcome(&outcome);
                if status == FileStatus::Succeeded {
                    state.files[index].position_us = state.files[index].duration_us;
                }
                state.files[index].status = status;
                state.files[index].eta = None;
                terminal[index] = Some(outcome);
                emit(event_tx, BatchEvent::FileFinished {
                    id: JobId(index),
                    path: state.files[index].path.clone(),
                    status,
                });
                if failed && options.failure_policy == FailurePolicy::FailFast {
                    fail_fast_triggered = true;
                    stop_admission = true;
                    phase_cancel.cancel();
                    cancel_execution_pending(
                        &mut pending,
                        terminal,
                        state,
                        event_tx,
                        CancellationReason::FailFast,
                    );
                }
                publish(snapshot_tx, state);
            }
        }
    }
    user_cancelled
}

async fn execute_one(
    runtime: Runtime,
    index: usize,
    plan: Arc<JobPlan>,
    tagged: mpsc::Sender<TaggedProgress>,
    cancel: CancellationToken,
) -> ExecutionTask {
    let (sender, mut receiver) = mpsc::channel(32);
    let execution = runtime.execute(plan, sender, cancel);
    tokio::pin!(execution);
    let result = loop {
        tokio::select! {
            result = &mut execution => break result,
            event = receiver.recv() => {
                if let Some(event) = event {
                    let _dropped = tagged.try_send(TaggedProgress { index, event });
                }
            }
        }
    };
    while let Ok(event) = receiver.try_recv() {
        let _dropped = tagged.try_send(TaggedProgress { index, event });
    }
    ExecutionTask { index, result }
}

fn cancel_pending(
    pending: &mut VecDeque<(usize, FileRequest)>,
    terminal: &mut [Option<FileOutcome>],
    state: &mut MutableState,
    event_tx: &broadcast::Sender<BatchEvent>,
    reason: CancellationReason,
) {
    for (index, _) in pending.drain(..) {
        terminal[index] = Some(FileOutcome::Cancelled(reason));
        state.files[index].status = FileStatus::Cancelled;
        emit(
            event_tx,
            BatchEvent::FileFinished {
                id: JobId(index),
                path: state.files[index].path.clone(),
                status: FileStatus::Cancelled,
            },
        );
    }
}

fn cancel_execution_pending(
    pending: &mut VecDeque<(usize, Arc<JobPlan>)>,
    terminal: &mut [Option<FileOutcome>],
    state: &mut MutableState,
    event_tx: &broadcast::Sender<BatchEvent>,
    reason: CancellationReason,
) {
    for (index, _) in pending.drain(..) {
        terminal[index] = Some(FileOutcome::Cancelled(reason));
        state.files[index].status = FileStatus::Cancelled;
        emit(
            event_tx,
            BatchEvent::FileFinished {
                id: JobId(index),
                path: state.files[index].path.clone(),
                status: FileStatus::Cancelled,
            },
        );
    }
}

fn finish_report(
    mut terminal: Vec<Option<FileOutcome>>,
    mut state: MutableState,
    event_tx: broadcast::Sender<BatchEvent>,
    snapshot_tx: watch::Sender<Arc<BatchSnapshot>>,
    user_cancelled: bool,
) -> BatchReport {
    let cancellation = user_cancelled.then_some(CancellationReason::User);
    let results = terminal
        .iter_mut()
        .enumerate()
        .map(|(index, outcome)| {
            let outcome = outcome.take().unwrap_or_else(|| {
                FileOutcome::Failed(FileFailure::new(
                    FailureStage::Internal,
                    "scheduler omitted a terminal file result",
                ))
            });
            FileResult {
                id: JobId(index),
                input: state.files[index].path.clone(),
                outcome,
            }
        })
        .collect();
    state.stage = if user_cancelled {
        BatchStage::Cancelled
    } else {
        BatchStage::Finished
    };
    publish(&snapshot_tx, &state);
    emit(
        &event_tx,
        if user_cancelled {
            BatchEvent::BatchCancelled
        } else {
            BatchEvent::BatchFinished
        },
    );
    BatchReport {
        results,
        cancellation,
    }
}

fn outcome_from_runtime(error: RuntimeError, stage: FailureStage) -> FileOutcome {
    if runtime_error_cancelled(&error) {
        FileOutcome::Cancelled(CancellationReason::User)
    } else {
        FileOutcome::Failed(FileFailure::new(stage, error.to_string()))
    }
}

fn runtime_error_cancelled(error: &RuntimeError) -> bool {
    matches!(
        error,
        RuntimeError::Backend(BackendError::Cancelled)
            | RuntimeError::Execution(ExecutionError::Cancelled)
    )
}

fn status_for_outcome(outcome: &FileOutcome) -> FileStatus {
    match outcome {
        FileOutcome::Succeeded(_) => FileStatus::Succeeded,
        FileOutcome::Skipped(_) => FileStatus::Skipped,
        FileOutcome::Planned(_) => FileStatus::Planned,
        FileOutcome::Failed(_) => FileStatus::Failed,
        FileOutcome::Cancelled(_) => FileStatus::Cancelled,
    }
}

fn publish(sender: &watch::Sender<Arc<BatchSnapshot>>, state: &MutableState) {
    sender.send_replace(state.snapshot());
}

fn emit(sender: &broadcast::Sender<BatchEvent>, event: BatchEvent) {
    let _ignored = sender.send(event);
}

fn bounded(mut message: String) -> String {
    const LIMIT: usize = 8 * 1024;
    if message.len() <= LIMIT {
        return message;
    }
    let mut boundary = LIMIT;
    while !message.is_char_boundary(boundary) {
        boundary = boundary.saturating_sub(1);
    }
    message.truncate(boundary);
    message.push('…');
    message
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests {
    use std::{
        collections::BTreeSet,
        io,
        path::{Path, PathBuf},
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
        time::{Duration, Instant},
    };

    use async_trait::async_trait;
    use sonicmux_backend::{
        BackendError, BackendExecution, BackendReport, MediaBackend, ProgressEvent,
    };
    use sonicmux_core::{AudioTarget, CompatibilityPolicy, OutputMode, ProfileName};
    use tempfile::tempdir;
    use tokio::{sync::Semaphore, time::timeout};
    use tokio_util::sync::CancellationToken;

    use super::{
        ActionRequest, AudioSelectionRequest, BatchRequest, BatchStage, CancellationReason,
        FailurePolicy, FileOutcome, FileRequest, FileStatus, MutableState, Runtime,
        SchedulerOptions,
    };

    #[derive(Clone)]
    struct MockBackend {
        active_probes: Arc<AtomicUsize>,
        maximum_probes: Arc<AtomicUsize>,
        active_executes: Arc<AtomicUsize>,
        maximum_executes: Arc<AtomicUsize>,
        execute_calls: Arc<AtomicUsize>,
        probe_gate: Arc<Semaphore>,
        execute_gate: Arc<Semaphore>,
        fail_probe: Arc<BTreeSet<PathBuf>>,
        fail_execute: Arc<BTreeSet<PathBuf>>,
    }

    impl MockBackend {
        fn open() -> Self {
            Self {
                active_probes: Arc::new(AtomicUsize::new(0)),
                maximum_probes: Arc::new(AtomicUsize::new(0)),
                active_executes: Arc::new(AtomicUsize::new(0)),
                maximum_executes: Arc::new(AtomicUsize::new(0)),
                execute_calls: Arc::new(AtomicUsize::new(0)),
                probe_gate: Arc::new(Semaphore::new(128)),
                execute_gate: Arc::new(Semaphore::new(128)),
                fail_probe: Arc::new(BTreeSet::new()),
                fail_execute: Arc::new(BTreeSet::new()),
            }
        }

        fn with_execute_gate(mut self, permits: usize) -> Self {
            self.execute_gate = Arc::new(Semaphore::new(permits));
            self
        }

        fn with_probe_gate(mut self, permits: usize) -> Self {
            self.probe_gate = Arc::new(Semaphore::new(permits));
            self
        }

        fn with_probe_failures(mut self, paths: impl IntoIterator<Item = PathBuf>) -> Self {
            self.fail_probe = Arc::new(paths.into_iter().collect());
            self
        }

        fn with_execute_failures(mut self, paths: impl IntoIterator<Item = PathBuf>) -> Self {
            self.fail_execute = Arc::new(paths.into_iter().collect());
            self
        }
    }

    struct ActiveGuard {
        counter: Arc<AtomicUsize>,
    }

    impl ActiveGuard {
        fn enter(counter: &Arc<AtomicUsize>, maximum: &AtomicUsize) -> Self {
            let active = counter.fetch_add(1, Ordering::SeqCst) + 1;
            maximum.fetch_max(active, Ordering::SeqCst);
            Self {
                counter: Arc::clone(counter),
            }
        }
    }

    impl Drop for ActiveGuard {
        fn drop(&mut self) {
            self.counter.fetch_sub(1, Ordering::SeqCst);
        }
    }

    #[async_trait]
    impl MediaBackend for MockBackend {
        async fn probe(
            &self,
            path: &Path,
            cancel: CancellationToken,
        ) -> Result<sonicmux_core::MediaInfo, BackendError> {
            let _active = ActiveGuard::enter(&self.active_probes, &self.maximum_probes);
            if self.fail_probe.contains(path) {
                return Err(BackendError::Probe {
                    path: path.to_path_buf(),
                    source: Box::new(io::Error::other("controlled probe failure")),
                });
            }
            tokio::select! {
                () = cancel.cancelled() => return Err(BackendError::Cancelled),
                permit = self.probe_gate.acquire() => {
                    let _permit = permit.map_err(|_| BackendError::Probe {
                        path: path.to_path_buf(),
                        source: Box::new(io::Error::other("probe gate closed")),
                    })?;
                }
            }
            sonicmux_ffmpeg::parse_probe_output(
                path.to_path_buf(),
                include_bytes!("../../ffmpeg/tests/fixtures/optional-fields.json"),
            )
            .map_err(|error| BackendError::Probe {
                path: path.to_path_buf(),
                source: Box::new(error),
            })
        }

        async fn execute(
            &self,
            request: BackendExecution,
            progress: tokio::sync::mpsc::Sender<ProgressEvent>,
            cancel: CancellationToken,
        ) -> Result<BackendReport, BackendError> {
            self.execute_calls.fetch_add(1, Ordering::SeqCst);
            let _active = ActiveGuard::enter(&self.active_executes, &self.maximum_executes);
            let _dropped = progress.try_send(ProgressEvent::Started);
            if self.fail_execute.contains(request.plan().input()) {
                return Err(BackendError::Execute {
                    source: Box::new(io::Error::other("controlled execution failure")),
                });
            }
            tokio::select! {
                () = cancel.cancelled() => return Err(BackendError::Cancelled),
                permit = self.execute_gate.acquire() => {
                    let _permit = permit.map_err(|_| BackendError::Execute {
                        source: Box::new(io::Error::other("execution gate closed")),
                    })?;
                }
            }
            tokio::fs::write(request.staging_path(), b"mock-matroska")
                .await
                .map_err(|error| BackendError::Execute {
                    source: Box::new(error),
                })?;
            Ok(BackendReport::new(
                Duration::from_millis(1),
                None,
                Vec::new(),
            ))
        }
    }

    fn file(input: impl Into<PathBuf>, output: PathBuf) -> FileRequest {
        FileRequest::new(
            input.into(),
            output,
            Arc::new(CompatibilityPolicy::for_profile(ProfileName::GenericTv)),
            AudioTarget::default(),
            OutputMode::Add,
            ActionRequest::RemuxOnly(AudioSelectionRequest::FirstCompatible),
        )
    }

    fn options(jobs: usize, policy: FailurePolicy) -> SchedulerOptions {
        SchedulerOptions::new(
            std::num::NonZeroUsize::new(jobs).expect("test concurrency is non-zero"),
            policy,
            false,
        )
        .expect("test scheduler options are valid")
    }

    async fn wait_for(counter: &AtomicUsize, expected: usize) {
        timeout(Duration::from_secs(2), async {
            while counter.load(Ordering::SeqCst) < expected {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("controlled worker count becomes observable");
    }

    #[tokio::test]
    async fn execution_is_bounded_and_report_order_is_stable() {
        let directory = tempdir().expect("temporary directory");
        let backend = MockBackend::open().with_execute_gate(0);
        let runtime = Runtime::new(Arc::new(backend.clone()));
        let requests = (0..3)
            .map(|index| {
                file(
                    format!("movie-{index}.mkv"),
                    directory.path().join(format!("output-{index}.mkv")),
                )
            })
            .collect();
        let handle = runtime.start_batch(
            BatchRequest::new(requests, options(2, FailurePolicy::Continue)),
            CancellationToken::new(),
        );
        wait_for(&backend.active_executes, 2).await;
        assert_eq!(backend.maximum_executes.load(Ordering::SeqCst), 2);
        backend.execute_gate.add_permits(3);
        let report = handle.wait().await.expect("batch supervisor succeeds");
        assert_eq!(report.results().len(), 3);
        assert!(report.results().iter().enumerate().all(|(index, result)| {
            result.id().get() == index && matches!(result.outcome(), FileOutcome::Succeeded(_))
        }));
        assert!(backend.maximum_executes.load(Ordering::SeqCst) <= 2);
    }

    #[tokio::test]
    async fn preparation_is_bounded_by_the_same_limit() {
        let directory = tempdir().expect("temporary directory");
        let backend = MockBackend::open().with_probe_gate(0);
        let runtime = Runtime::new(Arc::new(backend.clone()));
        let requests = (0..3)
            .map(|index| {
                file(
                    format!("probe-{index}.mkv"),
                    directory.path().join(format!("probe-output-{index}.mkv")),
                )
            })
            .collect();
        let dry_run = SchedulerOptions::new(
            std::num::NonZeroUsize::new(2).expect("test concurrency is non-zero"),
            FailurePolicy::Continue,
            true,
        )
        .expect("test scheduler options are valid");
        let handle = runtime.start_batch(
            BatchRequest::new(requests, dry_run),
            CancellationToken::new(),
        );
        wait_for(&backend.active_probes, 2).await;
        assert_eq!(backend.maximum_probes.load(Ordering::SeqCst), 2);
        backend.probe_gate.add_permits(6);
        let report = handle.wait().await.expect("batch supervisor succeeds");
        assert!(
            report
                .results()
                .iter()
                .all(|result| matches!(result.outcome(), FileOutcome::Planned(_)))
        );
        assert!(backend.maximum_probes.load(Ordering::SeqCst) <= 2);
    }

    #[tokio::test]
    async fn continue_policy_contains_one_file_failure() {
        let directory = tempdir().expect("temporary directory");
        let failed = PathBuf::from("bad.mkv");
        let backend = MockBackend::open().with_execute_failures([failed.clone()]);
        let runtime = Runtime::new(Arc::new(backend));
        let requests = vec![
            file("good-a.mkv", directory.path().join("a.mkv")),
            file(failed, directory.path().join("bad.mkv")),
            file("good-b.mkv", directory.path().join("b.mkv")),
        ];
        let report = runtime
            .start_batch(
                BatchRequest::new(requests, options(2, FailurePolicy::Continue)),
                CancellationToken::new(),
            )
            .wait()
            .await
            .expect("batch supervisor succeeds");
        assert_eq!(report.failure_count(), 1);
        assert_eq!(
            report
                .results()
                .iter()
                .filter(|result| matches!(result.outcome(), FileOutcome::Succeeded(_)))
                .count(),
            2
        );
    }

    #[tokio::test]
    async fn fail_fast_stops_preparation_admission() {
        let directory = tempdir().expect("temporary directory");
        let failed = PathBuf::from("first.mkv");
        let backend = MockBackend::open().with_probe_failures([failed.clone()]);
        let runtime = Runtime::new(Arc::new(backend.clone()));
        let requests = vec![
            file(failed, directory.path().join("first-output.mkv")),
            file("second.mkv", directory.path().join("second-output.mkv")),
            file("third.mkv", directory.path().join("third-output.mkv")),
        ];
        let report = runtime
            .start_batch(
                BatchRequest::new(requests, options(1, FailurePolicy::FailFast)),
                CancellationToken::new(),
            )
            .wait()
            .await
            .expect("batch supervisor succeeds");
        assert!(matches!(
            report.results()[0].outcome(),
            FileOutcome::Failed(_)
        ));
        assert!(report.results()[1..].iter().all(|result| matches!(
            result.outcome(),
            FileOutcome::Cancelled(CancellationReason::FailFast)
        )));
        assert_eq!(backend.execute_calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn duplicate_destinations_fail_before_execution() {
        let directory = tempdir().expect("temporary directory");
        let backend = MockBackend::open();
        let runtime = Runtime::new(Arc::new(backend.clone()));
        let output = directory.path().join("same.mkv");
        let report = runtime
            .start_batch(
                BatchRequest::new(
                    vec![file("a.mkv", output.clone()), file("b.mkv", output)],
                    options(2, FailurePolicy::Continue),
                ),
                CancellationToken::new(),
            )
            .wait()
            .await
            .expect("batch supervisor succeeds");
        assert!(
            report
                .results()
                .iter()
                .all(|result| matches!(result.outcome(), FileOutcome::Failed(_)))
        );
        assert_eq!(backend.execute_calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn fail_fast_duplicate_conflict_cancels_other_ready_jobs() {
        let directory = tempdir().expect("temporary directory");
        let backend = MockBackend::open();
        let runtime = Runtime::new(Arc::new(backend.clone()));
        let duplicate = directory.path().join("same.mkv");
        let report = runtime
            .start_batch(
                BatchRequest::new(
                    vec![
                        file("a.mkv", duplicate.clone()),
                        file("b.mkv", duplicate),
                        file("c.mkv", directory.path().join("unique.mkv")),
                    ],
                    options(3, FailurePolicy::FailFast),
                ),
                CancellationToken::new(),
            )
            .wait()
            .await
            .expect("batch supervisor succeeds");
        assert!(matches!(
            report.results()[0].outcome(),
            FileOutcome::Failed(_)
        ));
        assert!(matches!(
            report.results()[1].outcome(),
            FileOutcome::Failed(_)
        ));
        assert!(matches!(
            report.results()[2].outcome(),
            FileOutcome::Cancelled(CancellationReason::FailFast)
        ));
        assert_eq!(backend.execute_calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn fail_fast_execution_cancels_active_and_pending_jobs() {
        let directory = tempdir().expect("temporary directory");
        let failed = PathBuf::from("first.mkv");
        let backend = MockBackend::open()
            .with_execute_gate(0)
            .with_execute_failures([failed.clone()]);
        let runtime = Runtime::new(Arc::new(backend));
        let report = runtime
            .start_batch(
                BatchRequest::new(
                    vec![
                        file(failed, directory.path().join("first-output.mkv")),
                        file("second.mkv", directory.path().join("second-output.mkv")),
                        file("third.mkv", directory.path().join("third-output.mkv")),
                    ],
                    options(2, FailurePolicy::FailFast),
                ),
                CancellationToken::new(),
            )
            .wait()
            .await
            .expect("batch supervisor succeeds");
        assert!(matches!(
            report.results()[0].outcome(),
            FileOutcome::Failed(_)
        ));
        assert!(report.results()[1..].iter().all(|result| matches!(
            result.outcome(),
            FileOutcome::Cancelled(CancellationReason::FailFast)
        )));
    }

    #[tokio::test]
    async fn user_cancellation_waits_for_cleanup() {
        let directory = tempdir().expect("temporary directory");
        let backend = MockBackend::open().with_execute_gate(0);
        let runtime = Runtime::new(Arc::new(backend.clone()));
        let output = directory.path().join("cancelled.mkv");
        let cancel = CancellationToken::new();
        let handle = runtime.start_batch(
            BatchRequest::new(
                vec![file("long.mkv", output.clone())],
                options(1, FailurePolicy::Continue),
            ),
            cancel.clone(),
        );
        wait_for(&backend.active_executes, 1).await;
        cancel.cancel();
        let report = handle.wait().await.expect("batch supervisor succeeds");
        assert_eq!(report.cancellation(), Some(CancellationReason::User));
        assert!(matches!(
            report.results()[0].outcome(),
            FileOutcome::Cancelled(CancellationReason::User)
        ));
        assert!(!output.exists());
        let entries: Vec<_> = std::fs::read_dir(directory.path())
            .expect("temporary directory is readable")
            .collect::<Result<_, _>>()
            .expect("temporary entries are readable");
        assert!(entries.iter().all(|entry| {
            !entry
                .file_name()
                .to_string_lossy()
                .starts_with(".sonicmux-")
        }));
    }

    #[tokio::test]
    async fn dropping_event_receivers_does_not_change_truth() {
        let directory = tempdir().expect("temporary directory");
        let runtime = Runtime::new(Arc::new(MockBackend::open()));
        let handle = runtime.start_batch(
            BatchRequest::new(
                vec![file("movie.mkv", directory.path().join("output.mkv"))],
                options(1, FailurePolicy::Continue),
            ),
            CancellationToken::new(),
        );
        let (_, events, waiter) = handle.into_parts();
        drop(events);
        let report = waiter.wait().await.expect("batch supervisor succeeds");
        assert!(matches!(
            report.results()[0].outcome(),
            FileOutcome::Succeeded(_)
        ));
    }

    #[test]
    fn aggregate_progress_is_weighted_and_unknown_is_indeterminate() {
        let requests = vec![
            file("a.mkv", PathBuf::from("a.out.mkv")),
            file("b.mkv", PathBuf::from("b.out.mkv")),
        ];
        let mut state = MutableState::new(&requests);
        state.stage = BatchStage::Executing;
        state.execution_started = Some(Instant::now());
        state.files[0].will_execute = true;
        state.files[0].duration_us = Some(100);
        state.files[0].position_us = Some(50);
        state.files[0].status = FileStatus::Running;
        state.files[1].will_execute = true;
        assert_eq!(state.snapshot().progress_milli(), None);
        state.files[1].duration_us = Some(300);
        state.files[1].position_us = Some(150);
        state.files[1].status = FileStatus::Running;
        assert_eq!(state.snapshot().progress_milli(), Some(500));
    }
}
