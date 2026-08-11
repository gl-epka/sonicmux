//! Framework-neutral desktop session and scheduler orchestration.

use std::{
    collections::{HashSet, VecDeque},
    ffi::{OsStr, OsString},
    num::NonZeroUsize,
    path::{Path, PathBuf},
    sync::Arc,
};

use sonicmux_core::{
    AacBitrate, Ac3Bitrate, AudioTarget, CompatibilityPolicy, Eac3Bitrate, OutputMode,
    OutputStreamPlan, PlanOutcome, PlanningPolicy, RequestedAction, StreamInfo, TargetLayout,
};
use sonicmux_ffmpeg::{FfmpegCliBackend, ResolvedToolchain, resolve_toolchain_hybrid};
use sonicmux_runtime::{
    ActionRequest, AudioSelectionRequest, BatchReport, BatchRequest, BatchSnapshot, DefaultConfig,
    DiscoveryRequest, EffectiveConfig, FailurePolicy, FileOutcome, FileRequest, FileStatus,
    PartialConfig, Runtime, SchedulerOptions, discover, load_effective_config, merge_config,
    select_config_path,
};
use tauri::ipc::Channel;
use tokio::{
    sync::{Mutex, RwLock, Semaphore},
    task::JoinSet,
};
use tokio_util::sync::CancellationToken;

use crate::dto::{
    AcceptedDto, AppPhaseDto, BootstrapDto, GuiEventDto, QueueItemDto, SessionSnapshotDto,
    SettingsDto, ToolchainStatusDto, TrackDto,
};

const LOG_CAPACITY: usize = 300;
const PROBE_CONCURRENCY: usize = 4;
const GUI_SCHEMA: &str = "sonicmux.gui.v1";

/// Cloneable owner of one desktop application session.
#[derive(Clone)]
pub struct GuiService {
    inner: Arc<Inner>,
}

struct Inner {
    state: Mutex<Session>,
    runtime: RwLock<Option<Runtime>>,
    channel: RwLock<Option<Channel<GuiEventDto>>>,
    active_cancel: Mutex<Option<CancellationToken>>,
    root_cancel: CancellationToken,
}

#[derive(Debug, Clone)]
struct ProfileChoice {
    name: String,
    policy: CompatibilityPolicy,
}

#[derive(Debug, Clone)]
struct SessionSettings {
    dto: SettingsDto,
    output_directory: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ItemStatus {
    Probing,
    Ready,
    Compatible,
    Queued,
    Preparing,
    Running,
    Succeeded,
    Skipped,
    Planned,
    Failed,
    Cancelled,
}

impl ItemStatus {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Probing => "probing",
            Self::Ready => "ready",
            Self::Compatible => "compatible",
            Self::Queued => "queued",
            Self::Preparing => "preparing",
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Skipped => "skipped",
            Self::Planned => "planned",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    const fn can_start(self) -> bool {
        matches!(
            self,
            Self::Ready | Self::Compatible | Self::Succeeded | Self::Skipped | Self::Planned
        )
    }
}

#[derive(Debug, Clone)]
struct QueueItem {
    id: u64,
    input: PathBuf,
    output: PathBuf,
    enabled: bool,
    status: ItemStatus,
    progress_milli: Option<u16>,
    eta_seconds: Option<u64>,
    media: Option<sonicmux_core::MediaInfo>,
    plan: Option<PlanOutcome>,
    error: Option<String>,
}

struct Session {
    phase: AppPhaseDto,
    toolchain: ToolchainStatusDto,
    settings: SessionSettings,
    profiles: Vec<ProfileChoice>,
    queue: Vec<QueueItem>,
    next_id: u64,
    progress_milli: Option<u16>,
    eta_seconds: Option<u64>,
    active_ids: Vec<u64>,
    logs: VecDeque<String>,
}

impl GuiService {
    /// Loads effective configuration and resolves configured, bundled, or system tools.
    #[must_use]
    pub fn load(bundled_directory: Option<&Path>) -> Self {
        let (config, startup_error) = load_config();
        let explicit = config
            .ffmpeg_path
            .as_ref()
            .map(|value| value.value().as_path());
        let resolved = resolve_toolchain_hybrid(explicit, bundled_directory);
        let (runtime, toolchain) = match resolved {
            Ok(resolved) => {
                let status = toolchain_status(&resolved);
                let backend = FfmpegCliBackend::new(resolved.into_paths());
                (Some(Runtime::new(Arc::new(backend))), status)
            }
            Err(error) => (
                None,
                ToolchainStatusDto {
                    available: false,
                    source: "missing".to_owned(),
                    detail: format!(
                        "{error}. Choose an FFmpeg installation or install it, then retry."
                    ),
                },
            ),
        };
        let (profiles, settings) = settings_from_config(&config);
        let phase = if runtime.is_some() {
            AppPhaseDto::Idle
        } else {
            AppPhaseDto::ToolchainSetup
        };
        let mut logs = VecDeque::new();
        if let Some(error) = startup_error {
            logs.push_back(format!("configuration fallback: {error}"));
        }
        logs.push_back(if toolchain.available {
            format!("FFmpeg ready from {}", toolchain.source)
        } else {
            "FFmpeg setup required".to_owned()
        });
        Self {
            inner: Arc::new(Inner {
                state: Mutex::new(Session {
                    phase,
                    toolchain,
                    settings,
                    profiles,
                    queue: Vec::new(),
                    next_id: 0,
                    progress_milli: None,
                    eta_seconds: None,
                    active_ids: Vec::new(),
                    logs,
                }),
                runtime: RwLock::new(runtime),
                channel: RwLock::new(None),
                active_cancel: Mutex::new(None),
                root_cancel: CancellationToken::new(),
            }),
        }
    }

    /// Registers a replacement frontend channel and returns current truth.
    pub async fn bootstrap(&self, channel: Channel<GuiEventDto>) -> BootstrapDto {
        *self.inner.channel.write().await = Some(channel);
        let state = self.inner.state.lock().await;
        BootstrapDto {
            schema: GUI_SCHEMA,
            version: env!("CARGO_PKG_VERSION"),
            toolchain: state.toolchain.clone(),
            snapshot: state.snapshot(),
        }
    }

    /// Returns whether work is active or waiting for cleanup.
    pub async fn is_active(&self) -> bool {
        matches!(
            self.inner.state.lock().await.phase,
            AppPhaseDto::Running | AppPhaseDto::Cancelling
        )
    }

    /// Adds paths granted by a native picker or operating-system drop.
    pub async fn add_roots(&self, roots: Vec<PathBuf>) -> Result<SessionSnapshotDto, String> {
        if roots.is_empty() {
            return Ok(self.snapshot().await);
        }
        {
            let mut state = self.inner.state.lock().await;
            ensure_idle(&state)?;
            state.phase = AppPhaseDto::Probing;
            state.log(format!("discovering {} selected root(s)", roots.len()));
        }
        self.publish_snapshot().await;

        let request = DiscoveryRequest {
            roots: roots.into_iter().map(PathBuf::into_os_string).collect(),
            recursive: false,
            follow_links: false,
            includes: Vec::new(),
            excludes: Vec::new(),
        };
        let discovered = match discover(request).await {
            Ok(paths) => paths,
            Err(error) => {
                self.finish_probe_phase(Some(error.to_string())).await;
                return Err(error.to_string());
            }
        };
        let pending = {
            let mut state = self.inner.state.lock().await;
            let existing = state
                .queue
                .iter()
                .map(|item| item.input.clone())
                .collect::<HashSet<_>>();
            let mut pending = Vec::new();
            for input in discovered
                .into_iter()
                .filter(|path| !existing.contains(path))
            {
                let id = state.next_id;
                state.next_id = state.next_id.saturating_add(1);
                let output = default_output(&input, state.settings.output_directory.as_deref());
                state.queue.push(QueueItem {
                    id,
                    input: input.clone(),
                    output,
                    enabled: true,
                    status: ItemStatus::Probing,
                    progress_milli: None,
                    eta_seconds: None,
                    media: None,
                    plan: None,
                    error: None,
                });
                pending.push((id, input));
            }
            state.log(format!(
                "{} new MKV file(s) queued for probe",
                pending.len()
            ));
            pending
        };
        self.publish_snapshot().await;

        let Some(runtime) = self.inner.runtime.read().await.clone() else {
            let message = "FFmpeg is unavailable. Choose an installation, then retry these files.";
            let mut state = self.inner.state.lock().await;
            for item in &mut state.queue {
                if item.status == ItemStatus::Probing {
                    item.status = ItemStatus::Failed;
                    item.error = Some(message.to_owned());
                }
            }
            state.phase = AppPhaseDto::ToolchainSetup;
            drop(state);
            self.publish_snapshot().await;
            return Err(message.to_owned());
        };

        let limit = Arc::new(Semaphore::new(PROBE_CONCURRENCY));
        let mut tasks = JoinSet::new();
        for (id, input) in pending {
            let runtime = runtime.clone();
            let limit = Arc::clone(&limit);
            let cancel = self.inner.root_cancel.child_token();
            tasks.spawn(async move {
                let result = match limit.acquire_owned().await {
                    Ok(_permit) => runtime
                        .probe(&input, cancel)
                        .await
                        .map_err(|error| error.to_string()),
                    Err(error) => Err(format!("probe concurrency gate closed: {error}")),
                };
                (id, result)
            });
        }
        while let Some(joined) = tasks.join_next().await {
            match joined {
                Ok((id, result)) => self.apply_probe_result(id, result).await,
                Err(error) => {
                    self.notice(
                        "error",
                        format!("A probe task stopped unexpectedly: {error}. Retry the item."),
                    )
                    .await;
                }
            }
        }
        self.finish_probe_phase(None).await;
        Ok(self.snapshot().await)
    }

    /// Removes idle queue items by opaque session identifier.
    pub async fn remove_items(&self, ids: &[u64]) -> Result<SessionSnapshotDto, String> {
        let mut state = self.inner.state.lock().await;
        ensure_idle(&state)?;
        let ids = ids.iter().copied().collect::<HashSet<_>>();
        state.queue.retain(|item| !ids.contains(&item.id));
        state.log(format!("removed {} queue item(s)", ids.len()));
        let snapshot = state.snapshot();
        drop(state);
        self.publish_snapshot().await;
        Ok(snapshot)
    }

    /// Enables or disables one idle queue item.
    pub async fn set_item_enabled(
        &self,
        id: u64,
        enabled: bool,
    ) -> Result<SessionSnapshotDto, String> {
        let mut state = self.inner.state.lock().await;
        ensure_idle(&state)?;
        let item = state
            .queue
            .iter_mut()
            .find(|item| item.id == id)
            .ok_or_else(|| format!("queue item {id} no longer exists"))?;
        item.enabled = enabled;
        let snapshot = state.snapshot();
        drop(state);
        self.publish_snapshot().await;
        Ok(snapshot)
    }

    /// Validates settings and rebuilds cached pure plans.
    pub async fn update_settings(
        &self,
        mut input: SettingsDto,
    ) -> Result<SessionSnapshotDto, String> {
        let mut state = self.inner.state.lock().await;
        ensure_idle(&state)?;
        input.output_directory = state.settings.dto.output_directory.clone();
        validate_settings(&input, &state.profiles)?;
        state.settings.dto = input;
        rebuild_all_plans(&mut state);
        state.log("session settings updated; plans rebuilt".to_owned());
        let snapshot = state.snapshot();
        drop(state);
        self.publish_snapshot().await;
        Ok(snapshot)
    }

    /// Applies a native output-directory grant and rebuilds plans.
    pub async fn set_output_directory(
        &self,
        directory: PathBuf,
    ) -> Result<SessionSnapshotDto, String> {
        if !directory.is_dir() {
            return Err(format!(
                "output directory does not exist: {}",
                directory.display()
            ));
        }
        let mut state = self.inner.state.lock().await;
        ensure_idle(&state)?;
        state.settings.output_directory = Some(directory.clone());
        state.settings.dto.output_directory = Some(directory.to_string_lossy().into_owned());
        rebuild_all_plans(&mut state);
        state.log("output directory changed; plans rebuilt".to_owned());
        let snapshot = state.snapshot();
        drop(state);
        self.publish_snapshot().await;
        Ok(snapshot)
    }

    /// Resolves an explicitly selected FFmpeg executable and its sibling FFprobe.
    pub async fn choose_ffmpeg(&self, executable: PathBuf) -> Result<ToolchainStatusDto, String> {
        let resolved = resolve_toolchain_hybrid(Some(&executable), None)
            .map_err(|error| format!("{error}. Choose the ffmpeg executable next to ffprobe."))?;
        let status = toolchain_status(&resolved);
        let runtime = Runtime::new(Arc::new(FfmpegCliBackend::new(resolved.into_paths())));
        *self.inner.runtime.write().await = Some(runtime);
        let mut state = self.inner.state.lock().await;
        state.toolchain = status.clone();
        if state.phase == AppPhaseDto::ToolchainSetup {
            state.phase = AppPhaseDto::Idle;
        }
        state.log("FFmpeg installation selected for this session".to_owned());
        drop(state);
        self.publish_snapshot().await;
        Ok(status)
    }

    /// Rechecks the process `PATH` for a complete FFmpeg and FFprobe pair.
    pub async fn retry_system_toolchain(&self) -> Result<ToolchainStatusDto, String> {
        let resolved = resolve_toolchain_hybrid(None, None)
            .map_err(|error| format!("{error}. Install both FFmpeg and FFprobe, then retry."))?;
        let status = toolchain_status(&resolved);
        let runtime = Runtime::new(Arc::new(FfmpegCliBackend::new(resolved.into_paths())));
        *self.inner.runtime.write().await = Some(runtime);
        let mut state = self.inner.state.lock().await;
        state.toolchain = status.clone();
        if state.phase == AppPhaseDto::ToolchainSetup {
            state.phase = AppPhaseDto::Idle;
        }
        state.log("system FFmpeg search succeeded".to_owned());
        drop(state);
        self.publish_snapshot().await;
        Ok(status)
    }

    /// Starts the M5 scheduler and streams authoritative snapshots.
    pub async fn start_batch(&self) -> Result<AcceptedDto, String> {
        let runtime = self
            .inner
            .runtime
            .read()
            .await
            .clone()
            .ok_or_else(|| "FFmpeg setup is required before starting".to_owned())?;
        let request = {
            let mut state = self.inner.state.lock().await;
            ensure_idle(&state)?;
            if !state.can_start() {
                return Err(
                    "Enable at least one ready item or resolve its probe error before starting."
                        .to_owned(),
                );
            }
            let target = state.settings.target()?;
            let output_mode = state.settings.output_mode()?;
            let compatibility = Arc::new(state.settings.policy(&state.profiles));
            let action = if state.settings.dto.action == "remux" {
                ActionRequest::RemuxOnly(AudioSelectionRequest::FirstCompatible)
            } else {
                ActionRequest::Convert
            };
            let mut requests = Vec::new();
            let mut active_ids = Vec::new();
            for item in state.queue.iter_mut().filter(|item| item.enabled) {
                active_ids.push(item.id);
                item.status = ItemStatus::Queued;
                item.progress_milli = None;
                item.eta_seconds = None;
                item.error = None;
                requests.push(FileRequest::new(
                    item.input.clone(),
                    item.output.clone(),
                    Arc::clone(&compatibility),
                    target.clone(),
                    output_mode,
                    action.clone(),
                ));
            }
            state.active_ids = active_ids;
            let jobs = NonZeroUsize::new(state.settings.dto.jobs)
                .ok_or_else(|| "jobs must be at least one".to_owned())?;
            let options = SchedulerOptions::new(
                jobs,
                state.settings.failure_policy()?,
                state.settings.dto.dry_run,
            )
            .map_err(|error| error.to_string())?;
            state.phase = AppPhaseDto::Running;
            state.progress_milli = Some(0);
            state.eta_seconds = None;
            state.log(format!("batch started with {} file(s)", requests.len()));
            BatchRequest::new(requests, options)
        };
        self.publish_snapshot().await;
        let cancel = self.inner.root_cancel.child_token();
        *self.inner.active_cancel.lock().await = Some(cancel.clone());
        let handle = runtime.start_batch(request, cancel);
        let (mut snapshots, _events, waiter) = handle.into_parts();

        let snapshot_service = self.clone();
        tauri::async_runtime::spawn(async move {
            let first = snapshots.borrow().clone();
            snapshot_service.apply_batch_snapshot(&first).await;
            while snapshots.changed().await.is_ok() {
                let current = snapshots.borrow().clone();
                snapshot_service.apply_batch_snapshot(&current).await;
            }
        });
        let waiter_service = self.clone();
        tauri::async_runtime::spawn(async move {
            match waiter.wait().await {
                Ok(report) => waiter_service.finish_batch(report).await,
                Err(error) => {
                    waiter_service
                        .fail_batch(format!(
                            "Scheduler failed: {error}. Retry the affected files."
                        ))
                        .await;
                }
            }
        });
        Ok(AcceptedDto::yes())
    }

    /// Requests cooperative cancellation and retains the window until cleanup.
    pub async fn cancel_batch(&self) -> Result<AcceptedDto, String> {
        {
            let mut state = self.inner.state.lock().await;
            if state.phase != AppPhaseDto::Running {
                return Err("no active batch to cancel".to_owned());
            }
            state.phase = AppPhaseDto::Cancelling;
            state.log("cancellation requested; waiting for cleanup".to_owned());
        }
        if let Some(cancel) = self.inner.active_cancel.lock().await.as_ref() {
            cancel.cancel();
        }
        self.publish_snapshot().await;
        Ok(AcceptedDto::yes())
    }

    /// Replans failed or cancelled cached items, or probes them again.
    pub async fn retry_items(&self, ids: &[u64]) -> Result<SessionSnapshotDto, String> {
        let ids = ids.iter().copied().collect::<HashSet<_>>();
        let runtime = self.inner.runtime.read().await.clone();
        let reprobe = {
            let mut state = self.inner.state.lock().await;
            ensure_idle(&state)?;
            let mut reprobe = Vec::new();
            for item in &mut state.queue {
                if ids.contains(&item.id)
                    && matches!(item.status, ItemStatus::Failed | ItemStatus::Cancelled)
                {
                    item.enabled = true;
                    item.error = None;
                    if item.media.is_some() {
                        item.status = ItemStatus::Ready;
                    } else {
                        item.status = ItemStatus::Probing;
                        reprobe.push((item.id, item.input.clone()));
                    }
                }
            }
            rebuild_all_plans(&mut state);
            if !reprobe.is_empty() {
                state.phase = AppPhaseDto::Probing;
            }
            reprobe
        };
        self.publish_snapshot().await;
        if !reprobe.is_empty() {
            let Some(runtime) = runtime else {
                let message = "Choose FFmpeg before retrying probes".to_owned();
                self.finish_probe_phase(Some(message.clone())).await;
                return Err(message);
            };
            for (id, input) in reprobe {
                let result = runtime
                    .probe(&input, self.inner.root_cancel.child_token())
                    .await
                    .map_err(|error| error.to_string());
                self.apply_probe_result(id, result).await;
            }
            self.finish_probe_phase(None).await;
        }
        Ok(self.snapshot().await)
    }

    async fn apply_probe_result(&self, id: u64, result: Result<sonicmux_core::MediaInfo, String>) {
        let mut state = self.inner.state.lock().await;
        let settings = state.settings.clone();
        let profiles = state.profiles.clone();
        if let Some(item) = state.queue.iter_mut().find(|item| item.id == id) {
            match result {
                Ok(media) => {
                    item.media = Some(media);
                    rebuild_item_plan(&settings, &profiles, item);
                }
                Err(error) => {
                    item.status = ItemStatus::Failed;
                    item.error = Some(format!("{error}. Verify the MKV and retry."));
                }
            }
        }
        drop(state);
        self.publish_snapshot().await;
    }

    async fn finish_probe_phase(&self, error: Option<String>) {
        let runtime_available = self.inner.runtime.read().await.is_some();
        let mut state = self.inner.state.lock().await;
        state.phase = if runtime_available {
            AppPhaseDto::Idle
        } else {
            AppPhaseDto::ToolchainSetup
        };
        if let Some(error) = error {
            state.log(format!("discovery failed: {error}"));
        } else {
            state.log("probe pass finished".to_owned());
        }
        drop(state);
        self.publish_snapshot().await;
    }

    async fn apply_batch_snapshot(&self, snapshot: &BatchSnapshot) {
        let mut state = self.inner.state.lock().await;
        state.progress_milli = snapshot.progress_milli();
        state.eta_seconds = snapshot.eta().map(|value| value.as_secs());
        let active_ids = state.active_ids.clone();
        for file in snapshot.files() {
            if let Some(id) = active_ids.get(file.id().get())
                && let Some(item) = state.queue.iter_mut().find(|item| item.id == *id)
            {
                item.status = item_status(file.status());
                item.progress_milli = match (file.position_us(), file.duration_us()) {
                    (Some(position), Some(duration)) if duration > 0 => {
                        Some(((position.saturating_mul(1_000) / duration).min(1_000)) as u16)
                    }
                    _ => None,
                };
                item.eta_seconds = file.eta().map(|value| value.as_secs());
            }
        }
        drop(state);
        self.publish_snapshot().await;
    }

    async fn finish_batch(&self, report: BatchReport) {
        let mut state = self.inner.state.lock().await;
        let active_ids = state.active_ids.clone();
        for result in report.results() {
            if let Some(id) = active_ids.get(result.id().get())
                && let Some(item) = state.queue.iter_mut().find(|item| item.id == *id)
            {
                item.progress_milli = Some(1_000);
                item.eta_seconds = Some(0);
                match result.outcome() {
                    FileOutcome::Succeeded(_) => item.status = ItemStatus::Succeeded,
                    FileOutcome::Skipped(_) => item.status = ItemStatus::Skipped,
                    FileOutcome::Planned(_) => item.status = ItemStatus::Planned,
                    FileOutcome::Failed(failure) => {
                        item.status = ItemStatus::Failed;
                        item.error = Some(format!(
                            "{} failed: {}. Fix the cause and retry.",
                            failure.stage().as_str(),
                            failure.message()
                        ));
                    }
                    FileOutcome::Cancelled(_) => item.status = ItemStatus::Cancelled,
                    _ => {
                        item.status = ItemStatus::Failed;
                        item.error = Some("Unknown scheduler result. Retry the item.".to_owned());
                    }
                }
            }
        }
        state.phase = AppPhaseDto::Idle;
        state.progress_milli = Some(1_000);
        state.eta_seconds = Some(0);
        state.active_ids.clear();
        state.log(format!(
            "batch finished with {} failure(s)",
            report.failure_count()
        ));
        drop(state);
        *self.inner.active_cancel.lock().await = None;
        self.publish_snapshot().await;
    }

    async fn fail_batch(&self, message: String) {
        let mut state = self.inner.state.lock().await;
        state.phase = AppPhaseDto::Idle;
        state.log(message.clone());
        for item in &mut state.queue {
            if matches!(
                item.status,
                ItemStatus::Queued | ItemStatus::Preparing | ItemStatus::Running
            ) {
                item.status = ItemStatus::Failed;
                item.error = Some(message.clone());
            }
        }
        drop(state);
        *self.inner.active_cancel.lock().await = None;
        self.notice("error", message).await;
        self.publish_snapshot().await;
    }

    async fn snapshot(&self) -> SessionSnapshotDto {
        self.inner.state.lock().await.snapshot()
    }

    /// Returns the latest authoritative frontend snapshot.
    pub async fn current_snapshot(&self) -> SessionSnapshotDto {
        self.snapshot().await
    }

    async fn publish_snapshot(&self) {
        let snapshot = self.snapshot().await;
        let channel = self.inner.channel.read().await.clone();
        if let Some(channel) = channel {
            let _ignored = channel.send(GuiEventDto::Snapshot(Box::new(snapshot)));
        }
    }

    async fn notice(&self, level: &str, message: String) {
        let channel = self.inner.channel.read().await.clone();
        if let Some(channel) = channel {
            let _ignored = channel.send(GuiEventDto::Notice {
                level: level.to_owned(),
                message,
            });
        }
    }

    /// Forwards a stable native-menu action through the ordered GUI channel.
    pub async fn send_menu_action(&self, action: &str) {
        let channel = self.inner.channel.read().await.clone();
        if let Some(channel) = channel {
            let _ignored = channel.send(GuiEventDto::Menu {
                action: action.to_owned(),
            });
        }
    }
}

impl Session {
    fn snapshot(&self) -> SessionSnapshotDto {
        SessionSnapshotDto {
            phase: self.phase,
            queue: self.queue.iter().map(QueueItem::dto).collect(),
            settings: self.settings.dto.clone(),
            profiles: self
                .profiles
                .iter()
                .map(|value| value.name.clone())
                .collect(),
            can_start: self.can_start(),
            progress_milli: self.progress_milli,
            eta_seconds: self.eta_seconds,
            logs: self.logs.iter().cloned().collect(),
        }
    }

    fn can_start(&self) -> bool {
        self.phase == AppPhaseDto::Idle
            && self.queue.iter().any(|item| item.enabled)
            && self
                .queue
                .iter()
                .filter(|item| item.enabled)
                .all(|item| item.status.can_start())
    }

    fn log(&mut self, message: String) {
        if self.logs.len() == LOG_CAPACITY {
            self.logs.pop_front();
        }
        self.logs.push_back(message);
    }
}

impl SessionSettings {
    fn policy(&self, profiles: &[ProfileChoice]) -> CompatibilityPolicy {
        profiles
            .iter()
            .find(|choice| choice.name == self.dto.profile)
            .map_or_else(
                || CompatibilityPolicy::for_profile(sonicmux_core::ProfileName::GenericTv),
                |choice| choice.policy.clone(),
            )
    }

    fn target(&self) -> Result<AudioTarget, String> {
        let bitrate = parse_bitrate(&self.dto.bitrate)?;
        let layout = match self.dto.channels.as_str() {
            "keep-up-to-5.1" => TargetLayout::KeepUpTo51,
            "stereo" => TargetLayout::Stereo,
            "5.1" => TargetLayout::Surround51,
            value => return Err(format!("unsupported channel layout `{value}`")),
        };
        match self.dto.codec.as_str() {
            "ac3" => Ac3Bitrate::new(bitrate)
                .map(|bitrate| AudioTarget::Ac3 { bitrate, layout })
                .map_err(|error| error.to_string()),
            "eac3" => Eac3Bitrate::new(bitrate)
                .map(|bitrate| AudioTarget::Eac3 { bitrate, layout })
                .map_err(|error| error.to_string()),
            "aac" => AacBitrate::new(bitrate)
                .map(|bitrate| AudioTarget::Aac { bitrate, layout })
                .map_err(|error| error.to_string()),
            value => Err(format!("unsupported target codec `{value}`")),
        }
    }

    fn output_mode(&self) -> Result<OutputMode, String> {
        match self.dto.mode.as_str() {
            "add" => Ok(OutputMode::Add),
            "replace" => Ok(OutputMode::Replace),
            "only-new" => Ok(OutputMode::OnlyNew),
            value => Err(format!("unsupported output mode `{value}`")),
        }
    }

    fn failure_policy(&self) -> Result<FailurePolicy, String> {
        match self.dto.failure_policy.as_str() {
            "continue" => Ok(FailurePolicy::Continue),
            "fail-fast" => Ok(FailurePolicy::FailFast),
            value => Err(format!("unsupported failure policy `{value}`")),
        }
    }
}

impl QueueItem {
    fn dto(&self) -> QueueItemDto {
        QueueItemDto {
            id: self.id,
            name: self
                .input
                .file_name()
                .map_or_else(|| self.input.to_string_lossy(), OsStr::to_string_lossy)
                .into_owned(),
            input_display: self.input.to_string_lossy().into_owned(),
            output_display: self.output.to_string_lossy().into_owned(),
            enabled: self.enabled,
            status: self.status.as_str().to_owned(),
            progress_milli: self.progress_milli,
            eta_seconds: self.eta_seconds,
            plan: plan_summary(self.plan.as_ref()),
            error: self.error.clone(),
            tracks: self
                .media
                .as_ref()
                .map_or_else(Vec::new, |media| track_dtos(media, self.plan.as_ref())),
        }
    }
}

fn load_config() -> (EffectiveConfig, Option<String>) {
    match select_config_path(None)
        .and_then(|path| load_effective_config(&path, PartialConfig::default()))
    {
        Ok(config) => (config, None),
        Err(error) => {
            let fallback = merge_config(
                DefaultConfig::default(),
                PartialConfig::default(),
                PartialConfig::default(),
                PartialConfig::default(),
            );
            match fallback {
                Ok(config) => (config, Some(error.to_string())),
                Err(fallback_error) => {
                    tracing::error!(%fallback_error, "built-in GUI configuration is invalid");
                    std::process::exit(1);
                }
            }
        }
    }
}

fn settings_from_config(config: &EffectiveConfig) -> (Vec<ProfileChoice>, SessionSettings) {
    let profiles = config
        .profile_names()
        .filter_map(|name| {
            config
                .compatibility_policy_named(name)
                .ok()
                .map(|policy| ProfileChoice {
                    name: name.to_owned(),
                    policy,
                })
        })
        .collect::<Vec<_>>();
    let output_directory = config
        .output_directory
        .as_ref()
        .map(|value| value.value().clone());
    let dto = SettingsDto {
        profile: config.profile.value().clone(),
        action: "convert".to_owned(),
        codec: config.codec.value().clone(),
        bitrate: config.bitrate.value().clone(),
        channels: config.channels.value().clone(),
        mode: config.mode.value().clone(),
        jobs: *config.jobs.value(),
        storage_profile: config.storage_profile.value().clone(),
        failure_policy: "continue".to_owned(),
        dry_run: false,
        output_directory: output_directory
            .as_ref()
            .map(|path| path.to_string_lossy().into_owned()),
    };
    (
        profiles,
        SessionSettings {
            dto,
            output_directory,
        },
    )
}

fn toolchain_status(resolved: &ResolvedToolchain) -> ToolchainStatusDto {
    ToolchainStatusDto {
        available: true,
        source: resolved.source().as_str().to_owned(),
        detail: format!("FFmpeg: {}", resolved.paths().ffmpeg().to_string_lossy()),
    }
}

fn ensure_idle(state: &Session) -> Result<(), String> {
    if state.phase == AppPhaseDto::Idle || state.phase == AppPhaseDto::ToolchainSetup {
        Ok(())
    } else {
        Err("Wait for the active operation and cleanup to finish.".to_owned())
    }
}

fn validate_settings(input: &SettingsDto, profiles: &[ProfileChoice]) -> Result<(), String> {
    if !profiles.iter().any(|choice| choice.name == input.profile) {
        return Err(format!("unknown device profile `{}`", input.profile));
    }
    if !matches!(input.action.as_str(), "convert" | "remux") {
        return Err("action must be convert or remux".to_owned());
    }
    if !(1..=64).contains(&input.jobs) {
        return Err("jobs must be between 1 and 64".to_owned());
    }
    if !matches!(input.storage_profile.as_str(), "hdd" | "balanced" | "nvme") {
        return Err("storage profile must be hdd, balanced, or nvme".to_owned());
    }
    let settings = SessionSettings {
        dto: input.clone(),
        output_directory: None,
    };
    let _target = settings.target()?;
    let _mode = settings.output_mode()?;
    let _failure = settings.failure_policy()?;
    Ok(())
}

fn parse_bitrate(value: &str) -> Result<u64, String> {
    let normalized = value.trim().to_ascii_lowercase();
    if let Some(number) = normalized.strip_suffix('k') {
        return number
            .parse::<u64>()
            .ok()
            .and_then(|value| value.checked_mul(1_000))
            .ok_or_else(|| format!("invalid bitrate `{value}`"));
    }
    normalized
        .parse::<u64>()
        .map_err(|_| format!("invalid bitrate `{value}`"))
}

fn rebuild_all_plans(state: &mut Session) {
    let settings = state.settings.clone();
    let profiles = state.profiles.clone();
    for item in &mut state.queue {
        item.output = default_output(&item.input, settings.output_directory.as_deref());
        if item.media.is_some() {
            rebuild_item_plan(&settings, &profiles, item);
        }
    }
}

fn rebuild_item_plan(settings: &SessionSettings, profiles: &[ProfileChoice], item: &mut QueueItem) {
    let Some(media) = item.media.as_ref() else {
        return;
    };
    let target = match settings.target() {
        Ok(target) => target,
        Err(error) => {
            item.status = ItemStatus::Failed;
            item.error = Some(error);
            return;
        }
    };
    let mode = match settings.output_mode() {
        Ok(mode) => mode,
        Err(error) => {
            item.status = ItemStatus::Failed;
            item.error = Some(error);
            return;
        }
    };
    let action = if settings.dto.action == "remux" {
        RequestedAction::RemuxOnly {
            selection: sonicmux_core::AudioSelector::FirstCompatible,
        }
    } else {
        RequestedAction::Convert
    };
    let policy = PlanningPolicy::new(
        settings.policy(profiles),
        target,
        mode,
        action,
        item.output.clone(),
    );
    match sonicmux_core::build(media, &policy) {
        Ok(plan) => {
            item.status = match plan {
                PlanOutcome::Execute(_) => ItemStatus::Ready,
                PlanOutcome::Skip(_) => ItemStatus::Compatible,
                _ => ItemStatus::Ready,
            };
            item.plan = Some(plan);
            item.error = None;
        }
        Err(error) => {
            item.status = ItemStatus::Failed;
            item.plan = None;
            item.error = Some(format!("{error}. Adjust settings and retry."));
        }
    }
}

fn default_output(input: &Path, directory: Option<&Path>) -> PathBuf {
    let mut name = input
        .file_stem()
        .map_or_else(|| OsString::from("output"), OsString::from);
    name.push(".sonicmux.mkv");
    directory.map_or_else(
        || input.parent().unwrap_or_else(|| Path::new(".")).join(&name),
        |directory| directory.join(&name),
    )
}

fn item_status(status: FileStatus) -> ItemStatus {
    match status {
        FileStatus::Queued => ItemStatus::Queued,
        FileStatus::Preparing => ItemStatus::Preparing,
        FileStatus::Ready => ItemStatus::Ready,
        FileStatus::Running => ItemStatus::Running,
        FileStatus::Succeeded => ItemStatus::Succeeded,
        FileStatus::Skipped => ItemStatus::Skipped,
        FileStatus::Planned => ItemStatus::Planned,
        FileStatus::Failed => ItemStatus::Failed,
        FileStatus::Cancelled => ItemStatus::Cancelled,
    }
}

fn plan_summary(plan: Option<&PlanOutcome>) -> String {
    match plan {
        Some(PlanOutcome::Skip(_)) => "Already compatible".to_owned(),
        Some(PlanOutcome::Execute(plan)) => {
            let encoded = plan
                .streams()
                .iter()
                .filter(|stream| stream.is_encode())
                .count();
            if encoded == 0 {
                "Remux only".to_owned()
            } else {
                format!("Encode {encoded} audio track(s); copy video")
            }
        }
        _ => "Waiting for probe".to_owned(),
    }
}

fn track_dtos(media: &sonicmux_core::MediaInfo, plan: Option<&PlanOutcome>) -> Vec<TrackDto> {
    media
        .streams()
        .iter()
        .map(|stream| {
            let common = stream.common();
            let (kind, codec, channels) = match stream {
                StreamInfo::Video(_) => ("video", common.codec_name().to_owned(), None),
                StreamInfo::Audio(audio) => (
                    "audio",
                    audio.codec().to_string(),
                    Some(audio.channels().count().get()),
                ),
                StreamInfo::Subtitle(_) => ("subtitle", common.codec_name().to_owned(), None),
                StreamInfo::Attachment(_) => ("attachment", common.codec_name().to_owned(), None),
                StreamInfo::Data(_) => ("data", common.codec_name().to_owned(), None),
                StreamInfo::Unknown(value) => (value.kind(), common.codec_name().to_owned(), None),
                _ => ("unknown", common.codec_name().to_owned(), None),
            };
            TrackDto {
                index: common.index().get(),
                kind: kind.to_owned(),
                codec,
                channels,
                language: common.metadata().get("language").map(str::to_owned),
                title: common.metadata().get("title").map(str::to_owned),
                default: common.dispositions().is_default(),
                action: planned_action(plan, common.index()),
            }
        })
        .collect()
}

fn planned_action(plan: Option<&PlanOutcome>, source: sonicmux_core::StreamIndex) -> String {
    match plan {
        Some(PlanOutcome::Skip(_)) => "none".to_owned(),
        Some(PlanOutcome::Execute(plan)) => {
            let mut copy = false;
            let mut encode = false;
            for operation in plan
                .streams()
                .iter()
                .filter(|operation| operation.source() == source)
            {
                match operation {
                    OutputStreamPlan::Copy { .. } => copy = true,
                    OutputStreamPlan::EncodeAudio { .. } => encode = true,
                    _ => {}
                }
            }
            match (copy, encode) {
                (true, true) => "copy+encode",
                (false, true) => "encode",
                (true, false) => "copy",
                (false, false) => "omit",
            }
            .to_owned()
        }
        _ => "pending".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{ProfileChoice, SettingsDto, default_output, parse_bitrate, validate_settings};
    use sonicmux_core::{CompatibilityPolicy, ProfileName};

    fn settings() -> SettingsDto {
        SettingsDto {
            profile: "generic-tv".to_owned(),
            action: "convert".to_owned(),
            codec: "ac3".to_owned(),
            bitrate: "640k".to_owned(),
            channels: "keep-up-to-5.1".to_owned(),
            mode: "add".to_owned(),
            jobs: 2,
            storage_profile: "balanced".to_owned(),
            failure_policy: "continue".to_owned(),
            dry_run: false,
            output_directory: None,
        }
    }

    #[test]
    fn validates_bounded_session_settings() {
        let profiles = [ProfileChoice {
            name: "generic-tv".to_owned(),
            policy: CompatibilityPolicy::for_profile(ProfileName::GenericTv),
        }];
        assert!(validate_settings(&settings(), &profiles).is_ok());

        let mut invalid = settings();
        invalid.jobs = 0;
        assert_eq!(
            validate_settings(&invalid, &profiles),
            Err("jobs must be between 1 and 64".to_owned())
        );
    }

    #[test]
    fn parses_human_bitrates_and_derives_non_replacing_output() {
        assert_eq!(parse_bitrate("640k"), Ok(640_000));
        assert_eq!(
            parse_bitrate("oops"),
            Err("invalid bitrate `oops`".to_owned())
        );
        assert_eq!(
            default_output(Path::new("movies/input.mkv"), None),
            PathBuf::from("movies/input.sonicmux.mkv")
        );
    }
}
