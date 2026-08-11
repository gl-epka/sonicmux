use std::{
    collections::{HashSet, VecDeque},
    ffi::OsString,
    num::NonZeroUsize,
    path::{Path, PathBuf},
    sync::Arc,
};

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use sonicmux_core::{
    AacBitrate, Ac3Bitrate, AudioTarget, CompatibilityPolicy, Eac3Bitrate, MediaInfo, OutputMode,
    PlanOutcome, PlanningPolicy, RequestedAction, TargetLayout,
};
use sonicmux_runtime::{
    ActionRequest, AudioSelectionRequest, BatchEvent, BatchReport, BatchRequest, BatchSnapshot,
    DiscoveryError, DiscoveryRequest, EffectiveConfig, FailurePolicy, FileOutcome, FileRequest,
    FileStatus, RuntimeError, SchedulerError, SchedulerOptions, StorageProfile,
};

const LOG_CAPACITY: usize = 1_000;
const CODECS: &[&str] = &["ac3", "eac3", "aac"];
const CHANNELS: &[&str] = &["keep-up-to-5.1", "stereo", "5.1"];
const MODES: &[&str] = &["add", "replace", "only-new"];
const STORAGE: &[&str] = &["hdd", "balanced", "nvme"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct QueueId(u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Screen {
    Queue,
    Tracks,
    Logs,
    Settings,
}

impl Screen {
    pub(crate) const fn index(self) -> usize {
        match self {
            Self::Queue => 0,
            Self::Tracks => 1,
            Self::Logs => 2,
            Self::Settings => 3,
        }
    }

    const fn from_index(index: usize) -> Self {
        match index % 4 {
            0 => Self::Queue,
            1 => Self::Tracks,
            2 => Self::Logs,
            _ => Self::Settings,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AppPhase {
    Idle,
    Running,
    Cancelling,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Overlay {
    Help,
    PathEditor { value: String, cursor: usize },
    ConfirmCancel,
    Notice(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum QueueStatus {
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

impl QueueStatus {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Probing => "[PROBE]",
            Self::Ready => "[READY]",
            Self::Compatible => "[SKIP]",
            Self::Queued => "[WAIT]",
            Self::Preparing => "[PREP]",
            Self::Running => "[RUN]",
            Self::Succeeded => "[OK]",
            Self::Skipped => "[SKIP]",
            Self::Planned => "[PLAN]",
            Self::Failed => "[ERR]",
            Self::Cancelled => "[CXL]",
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
pub(crate) struct QueueItem {
    pub(crate) id: QueueId,
    pub(crate) input: PathBuf,
    pub(crate) output: PathBuf,
    pub(crate) enabled: bool,
    pub(crate) status: QueueStatus,
    pub(crate) progress_milli: Option<u16>,
    pub(crate) eta: Option<std::time::Duration>,
    pub(crate) media: Option<MediaInfo>,
    pub(crate) plan: Option<PlanOutcome>,
    pub(crate) error: Option<String>,
}

#[derive(Debug, Clone)]
struct ProfileChoice {
    name: String,
    policy: CompatibilityPolicy,
}

#[derive(Debug, Clone)]
pub(crate) struct UiSettings {
    profiles: Vec<ProfileChoice>,
    profile: usize,
    pub(crate) action_remux: bool,
    pub(crate) codec: String,
    pub(crate) bitrate: String,
    pub(crate) channels: String,
    pub(crate) mode: String,
    pub(crate) jobs: usize,
    pub(crate) storage_profile: String,
    pub(crate) failure_policy: FailurePolicy,
    pub(crate) dry_run: bool,
    pub(crate) output_directory: Option<PathBuf>,
    pub(crate) selected_field: usize,
}

impl UiSettings {
    pub(crate) fn from_config(
        config: &EffectiveConfig,
        dry_run: bool,
        output_directory: Option<PathBuf>,
    ) -> Result<Self, sonicmux_runtime::ConfigError> {
        let mut profiles = Vec::new();
        for name in config.profile_names() {
            profiles.push(ProfileChoice {
                name: name.to_owned(),
                policy: config.compatibility_policy_named(name)?,
            });
        }
        let profile = profiles
            .iter()
            .position(|candidate| candidate.name == *config.profile.value())
            .unwrap_or(0);
        Ok(Self {
            profiles,
            profile,
            action_remux: false,
            codec: config.codec.value().clone(),
            bitrate: config.bitrate.value().clone(),
            channels: config.channels.value().clone(),
            mode: config.mode.value().clone(),
            jobs: *config.jobs.value(),
            storage_profile: config.storage_profile.value().clone(),
            failure_policy: FailurePolicy::Continue,
            dry_run,
            output_directory,
            selected_field: 0,
        })
    }

    pub(crate) fn profile_name(&self) -> &str {
        self.profiles
            .get(self.profile)
            .map_or("generic-tv", |choice| choice.name.as_str())
    }

    fn policy(&self) -> CompatibilityPolicy {
        self.profiles.get(self.profile).map_or_else(
            || CompatibilityPolicy::for_profile(sonicmux_core::ProfileName::GenericTv),
            |choice| choice.policy.clone(),
        )
    }

    pub(crate) fn target(&self) -> Result<AudioTarget, String> {
        let bitrate = parse_bitrate(&self.bitrate)?;
        let layout = match self.channels.as_str() {
            "keep-up-to-5.1" => TargetLayout::KeepUpTo51,
            "stereo" => TargetLayout::Stereo,
            "5.1" => TargetLayout::Surround51,
            value => return Err(format!("unsupported channel layout `{value}`")),
        };
        match self.codec.as_str() {
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
        match self.mode.as_str() {
            "add" => Ok(OutputMode::Add),
            "replace" => Ok(OutputMode::Replace),
            "only-new" => Ok(OutputMode::OnlyNew),
            value => Err(format!("unsupported output mode `{value}`")),
        }
    }

    pub(crate) const fn field_count() -> usize {
        10
    }

    pub(crate) fn field_label(&self, index: usize) -> (&'static str, String) {
        match index {
            0 => ("Profile", self.profile_name().to_owned()),
            1 => (
                "Action",
                if self.action_remux {
                    "remux"
                } else {
                    "convert"
                }
                .to_owned(),
            ),
            2 => ("Codec", self.codec.clone()),
            3 => ("Bitrate", self.bitrate.clone()),
            4 => ("Channels", self.channels.clone()),
            5 => ("Mode", self.mode.clone()),
            6 => ("Jobs", self.jobs.to_string()),
            7 => ("Storage", self.storage_profile.clone()),
            8 => ("Failures", self.failure_policy.as_str().to_owned()),
            _ => (
                "Dry run",
                if self.dry_run { "yes" } else { "no" }.to_owned(),
            ),
        }
    }

    fn change(&mut self, direction: i32) {
        match self.selected_field {
            0 => self.profile = cycle_index(self.profile, self.profiles.len(), direction),
            1 => self.action_remux = !self.action_remux,
            2 => {
                self.codec = cycle_value(&self.codec, CODECS, direction);
                self.bitrate = default_bitrate(&self.codec).to_owned();
            }
            3 => self.bitrate = cycle_value(&self.bitrate, bitrate_values(&self.codec), direction),
            4 => self.channels = cycle_value(&self.channels, CHANNELS, direction),
            5 => self.mode = cycle_value(&self.mode, MODES, direction),
            6 => {
                self.jobs = if direction < 0 {
                    self.jobs.saturating_sub(1).max(1)
                } else {
                    self.jobs.saturating_add(1).min(64)
                };
            }
            7 => {
                self.storage_profile = cycle_value(&self.storage_profile, STORAGE, direction);
                if self.storage_profile == StorageProfile::Hdd.as_str() {
                    self.jobs = 1;
                }
            }
            8 => {
                self.failure_policy = match self.failure_policy {
                    FailurePolicy::Continue => FailurePolicy::FailFast,
                    FailurePolicy::FailFast => FailurePolicy::Continue,
                };
            }
            _ => self.dry_run = !self.dry_run,
        }
    }
}

#[derive(Debug)]
pub(crate) enum Msg {
    Input(KeyEvent),
    Paste(String),
    Resize,
    Tick,
    InputsDiscovered(Result<Vec<PathBuf>, DiscoveryError>),
    ProbeFinished(QueueId, Result<MediaInfo, RuntimeError>),
    BatchSnapshot(Arc<BatchSnapshot>),
    BatchEvent(BatchEvent),
    EventsLagged(u64),
    TaskFailed(String),
    BatchFinished(Result<BatchReport, SchedulerError>),
}

#[derive(Debug)]
pub(crate) enum Effect {
    Discover(DiscoveryRequest),
    Probe(QueueId, PathBuf),
    StartBatch,
    CancelBatch,
}

#[derive(Debug)]
pub(crate) struct Model {
    pub(crate) screen: Screen,
    pub(crate) phase: AppPhase,
    pub(crate) queue: Vec<QueueItem>,
    pub(crate) selected: Option<usize>,
    pub(crate) settings: UiSettings,
    pub(crate) snapshot: Option<Arc<BatchSnapshot>>,
    pub(crate) overlay: Option<Overlay>,
    pub(crate) logs: VecDeque<String>,
    pub(crate) should_quit: bool,
    pub(crate) color: bool,
    next_id: u64,
    active_queue_ids: Vec<QueueId>,
    discovery_template: DiscoveryRequest,
}

impl Model {
    pub(crate) fn new(
        settings: UiSettings,
        discovery_template: DiscoveryRequest,
        color: bool,
    ) -> Self {
        Self {
            screen: Screen::Queue,
            phase: AppPhase::Idle,
            queue: Vec::new(),
            selected: None,
            settings,
            snapshot: None,
            overlay: None,
            logs: VecDeque::new(),
            should_quit: false,
            color,
            next_id: 0,
            active_queue_ids: Vec::new(),
            discovery_template,
        }
    }

    pub(crate) fn startup_effect(&self) -> Option<Effect> {
        (!self.discovery_template.roots.is_empty())
            .then(|| Effect::Discover(self.discovery_template.clone()))
    }

    pub(crate) fn selected_item(&self) -> Option<&QueueItem> {
        self.selected.and_then(|index| self.queue.get(index))
    }

    pub(crate) fn overall_progress(&self) -> Option<u16> {
        self.snapshot
            .as_ref()
            .and_then(|value| value.progress_milli())
    }

    pub(crate) fn update(&mut self, message: Msg) -> Vec<Effect> {
        match message {
            Msg::Input(key) if key.kind == KeyEventKind::Press => return self.on_key(key),
            Msg::Paste(value) => self.on_paste(&value),
            Msg::Resize | Msg::Tick => {}
            Msg::InputsDiscovered(result) => return self.on_discovered(result),
            Msg::ProbeFinished(id, result) => self.on_probe(id, result),
            Msg::BatchSnapshot(snapshot) => self.on_snapshot(snapshot),
            Msg::BatchEvent(event) => self.on_batch_event(&event),
            Msg::EventsLagged(count) => self.log(format!(
                "event receiver lagged by {count}; recovered from the latest snapshot"
            )),
            Msg::TaskFailed(message) => {
                self.log(message.clone());
                self.overlay = Some(Overlay::Notice(message));
                if self.phase == AppPhase::Running {
                    self.phase = AppPhase::Cancelling;
                    return vec![Effect::CancelBatch];
                }
            }
            Msg::BatchFinished(result) => self.on_batch_finished(result),
            Msg::Input(_) => {}
        }
        Vec::new()
    }

    fn on_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        if let Some(overlay) = self.overlay.take() {
            return self.on_overlay_key(overlay, key);
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return self.request_cancel();
        }
        match key.code {
            KeyCode::Char('?') => self.overlay = Some(Overlay::Help),
            KeyCode::Char('1') => self.screen = Screen::Queue,
            KeyCode::Char('2') => self.screen = Screen::Tracks,
            KeyCode::Char('3') => self.screen = Screen::Logs,
            KeyCode::Char('4') => self.screen = Screen::Settings,
            KeyCode::Tab => self.screen = Screen::from_index(self.screen.index() + 1),
            KeyCode::BackTab => self.screen = Screen::from_index(self.screen.index() + 3),
            KeyCode::Char('q') => {
                if self.phase == AppPhase::Idle {
                    self.should_quit = true;
                } else {
                    self.overlay = Some(Overlay::ConfirmCancel);
                }
            }
            KeyCode::Char('a') if self.phase == AppPhase::Idle => {
                self.overlay = Some(Overlay::PathEditor {
                    value: String::new(),
                    cursor: 0,
                });
            }
            KeyCode::Char('s') if self.screen == Screen::Queue => {
                if self.can_start() {
                    return vec![Effect::StartBatch];
                }
                self.overlay = Some(Overlay::Notice(self.start_blocker()));
            }
            KeyCode::Char('c') => return self.request_cancel(),
            KeyCode::Char('r') if self.phase == AppPhase::Idle => {
                return self.retry_terminal();
            }
            KeyCode::Char('d') if self.phase == AppPhase::Idle && self.screen == Screen::Queue => {
                self.remove_selected();
            }
            KeyCode::Char(' ') if self.phase == AppPhase::Idle && self.screen == Screen::Queue => {
                if let Some(index) = self.selected {
                    if let Some(item) = self.queue.get_mut(index) {
                        item.enabled = !item.enabled;
                    }
                }
            }
            KeyCode::Down | KeyCode::Char('j') => self.move_selection(1),
            KeyCode::Up | KeyCode::Char('k') => self.move_selection(-1),
            KeyCode::Char('g') => self.select_first(),
            KeyCode::Char('G') => self.select_last(),
            KeyCode::Left | KeyCode::Char('h')
                if self.phase == AppPhase::Idle && self.screen == Screen::Settings =>
            {
                self.settings.change(-1);
                self.rebuild_plans();
            }
            KeyCode::Right | KeyCode::Enter | KeyCode::Char('l')
                if self.phase == AppPhase::Idle && self.screen == Screen::Settings =>
            {
                self.settings.change(1);
                self.rebuild_plans();
            }
            _ => {}
        }
        Vec::new()
    }

    fn on_overlay_key(&mut self, overlay: Overlay, key: KeyEvent) -> Vec<Effect> {
        match overlay {
            Overlay::Help | Overlay::Notice(_) => {
                if !matches!(
                    key.code,
                    KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') | KeyCode::Char('?')
                ) {
                    self.overlay = Some(overlay);
                }
            }
            Overlay::ConfirmCancel => match key.code {
                KeyCode::Char('y') | KeyCode::Enter => return self.request_cancel(),
                KeyCode::Char('n') | KeyCode::Esc | KeyCode::Char('q') => {}
                _ => self.overlay = Some(Overlay::ConfirmCancel),
            },
            Overlay::PathEditor {
                mut value,
                mut cursor,
            } => match key.code {
                KeyCode::Esc => {}
                KeyCode::Enter => {
                    if value.trim().is_empty() {
                        self.overlay = Some(Overlay::Notice(
                            "Path cannot be empty. Close this message and press a to try again."
                                .to_owned(),
                        ));
                    } else {
                        let mut request = self.discovery_template.clone();
                        request.roots = vec![OsString::from(value)];
                        return vec![Effect::Discover(request)];
                    }
                }
                KeyCode::Char(character) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    insert_char(&mut value, &mut cursor, character);
                    self.overlay = Some(Overlay::PathEditor { value, cursor });
                }
                KeyCode::Backspace => {
                    remove_previous_char(&mut value, &mut cursor);
                    self.overlay = Some(Overlay::PathEditor { value, cursor });
                }
                KeyCode::Delete => {
                    remove_next_char(&mut value, cursor);
                    self.overlay = Some(Overlay::PathEditor { value, cursor });
                }
                KeyCode::Left => {
                    cursor = previous_boundary(&value, cursor);
                    self.overlay = Some(Overlay::PathEditor { value, cursor });
                }
                KeyCode::Right => {
                    cursor = next_boundary(&value, cursor);
                    self.overlay = Some(Overlay::PathEditor { value, cursor });
                }
                KeyCode::Home => {
                    self.overlay = Some(Overlay::PathEditor { value, cursor: 0 });
                }
                KeyCode::End => {
                    cursor = value.len();
                    self.overlay = Some(Overlay::PathEditor { value, cursor });
                }
                _ => self.overlay = Some(Overlay::PathEditor { value, cursor }),
            },
        }
        Vec::new()
    }

    fn on_paste(&mut self, pasted: &str) {
        if let Some(Overlay::PathEditor {
            mut value,
            mut cursor,
        }) = self.overlay.take()
        {
            value.insert_str(cursor, pasted);
            cursor = cursor.saturating_add(pasted.len());
            self.overlay = Some(Overlay::PathEditor { value, cursor });
        }
    }

    fn on_discovered(&mut self, result: Result<Vec<PathBuf>, DiscoveryError>) -> Vec<Effect> {
        let files = match result {
            Ok(files) => files,
            Err(error) => {
                let message = error.to_string();
                self.log(format!("discovery failed: {message}"));
                self.overlay = Some(Overlay::Notice(message));
                return Vec::new();
            }
        };
        let known: HashSet<PathBuf> = self.queue.iter().map(|item| item.input.clone()).collect();
        let mut effects = Vec::new();
        for input in files {
            if known.contains(&input) || self.queue.iter().any(|item| item.input == input) {
                continue;
            }
            let id = QueueId(self.next_id);
            self.next_id = self.next_id.saturating_add(1);
            let output = default_output(&input, self.settings.output_directory.as_deref());
            self.queue.push(QueueItem {
                id,
                input: input.clone(),
                output,
                enabled: true,
                status: QueueStatus::Probing,
                progress_milli: None,
                eta: None,
                media: None,
                plan: None,
                error: None,
            });
            effects.push(Effect::Probe(id, input));
        }
        if self.selected.is_none() && !self.queue.is_empty() {
            self.selected = Some(0);
        }
        self.log(format!(
            "queue contains {} unique MKV file(s)",
            self.queue.len()
        ));
        effects
    }

    fn on_probe(&mut self, id: QueueId, result: Result<MediaInfo, RuntimeError>) {
        let Some(index) = self.queue.iter().position(|item| item.id == id) else {
            return;
        };
        match result {
            Ok(media) => {
                self.queue[index].media = Some(media);
                self.rebuild_plan(index);
                self.log(format!("probed {}", self.queue[index].input.display()));
            }
            Err(error) => {
                self.queue[index].status = QueueStatus::Failed;
                self.queue[index].error = Some(error.to_string());
                self.log(format!(
                    "probe failed for {}: {error}",
                    self.queue[index].input.display()
                ));
            }
        }
    }

    fn on_snapshot(&mut self, snapshot: Arc<BatchSnapshot>) {
        for file in snapshot.files() {
            if let Some(queue_id) = self.active_queue_ids.get(file.id().get()) {
                if let Some(item) = self.queue.iter_mut().find(|item| item.id == *queue_id) {
                    item.status = queue_status(file.status());
                    item.progress_milli = match (file.position_us(), file.duration_us()) {
                        (Some(position), Some(duration)) if duration > 0 => {
                            let milli = position.saturating_mul(1_000) / duration;
                            Some(u16::try_from(milli.min(1_000)).unwrap_or(1_000))
                        }
                        _ => None,
                    };
                    item.eta = file.eta();
                }
            }
        }
        self.snapshot = Some(snapshot);
    }

    fn on_batch_event(&mut self, event: &BatchEvent) {
        let message = match event {
            BatchEvent::BatchStarted {
                total, concurrency, ..
            } => {
                format!("batch started: {total} file(s), concurrency {concurrency}")
            }
            BatchEvent::PreparationStarted => "preparing files".to_owned(),
            BatchEvent::FileStarted { path, .. } => format!("preparing {}", path.display()),
            BatchEvent::FilePrepared { path, status, .. } => {
                format!("prepared {}: {}", path.display(), status.as_str())
            }
            BatchEvent::ExecutionStarted { ready } => format!("executing {ready} prepared file(s)"),
            BatchEvent::FileProgress { .. } => return,
            BatchEvent::FileFinished { path, status, .. } => {
                format!("finished {}: {}", path.display(), status.as_str())
            }
            BatchEvent::BatchFinished => "batch finished".to_owned(),
            BatchEvent::BatchCancelled => "batch cancellation cleanup finished".to_owned(),
            _ => "scheduler emitted a newer event".to_owned(),
        };
        self.log(message);
    }

    fn on_batch_finished(&mut self, result: Result<BatchReport, SchedulerError>) {
        self.phase = AppPhase::Idle;
        self.snapshot = None;
        match result {
            Ok(report) => {
                for result in report.results() {
                    let Some(queue_id) = self.active_queue_ids.get(result.id().get()) else {
                        continue;
                    };
                    let Some(item) = self.queue.iter_mut().find(|item| item.id == *queue_id) else {
                        continue;
                    };
                    item.progress_milli = Some(1_000);
                    item.eta = None;
                    match result.outcome() {
                        FileOutcome::Succeeded(_) => item.status = QueueStatus::Succeeded,
                        FileOutcome::Skipped(_) => item.status = QueueStatus::Skipped,
                        FileOutcome::Planned(_) => item.status = QueueStatus::Planned,
                        FileOutcome::Failed(failure) => {
                            item.status = QueueStatus::Failed;
                            item.error = Some(format!(
                                "{}: {}",
                                failure.stage().as_str(),
                                failure.message()
                            ));
                        }
                        FileOutcome::Cancelled(_) => item.status = QueueStatus::Cancelled,
                        _ => {}
                    }
                }
                let failures = report.failure_count();
                let cancelled = report.cancellation().is_some();
                self.log(format!(
                    "batch complete: {failures} failure(s), cancelled={cancelled}"
                ));
            }
            Err(error) => {
                self.log(format!("scheduler failed: {error}"));
                self.overlay = Some(Overlay::Notice(error.to_string()));
            }
        }
        self.active_queue_ids.clear();
    }

    fn request_cancel(&mut self) -> Vec<Effect> {
        if self.phase == AppPhase::Running {
            self.phase = AppPhase::Cancelling;
            self.log("cancellation requested; waiting for cleanup".to_owned());
            vec![Effect::CancelBatch]
        } else {
            Vec::new()
        }
    }

    pub(crate) fn build_batch(&mut self) -> Result<BatchRequest, String> {
        let target = self.settings.target()?;
        let output_mode = self.settings.output_mode()?;
        let compatibility = Arc::new(self.settings.policy());
        let action = if self.settings.action_remux {
            ActionRequest::RemuxOnly(AudioSelectionRequest::FirstCompatible)
        } else {
            ActionRequest::Convert
        };
        let mut requests = Vec::new();
        self.active_queue_ids.clear();
        for item in self.queue.iter_mut().filter(|item| item.enabled) {
            self.active_queue_ids.push(item.id);
            item.status = QueueStatus::Queued;
            item.progress_milli = None;
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
        let jobs = NonZeroUsize::new(self.settings.jobs)
            .ok_or_else(|| "scheduler jobs must be greater than zero".to_owned())?;
        let options =
            SchedulerOptions::new(jobs, self.settings.failure_policy, self.settings.dry_run)
                .map_err(|error| error.to_string())?;
        self.phase = AppPhase::Running;
        Ok(BatchRequest::new(requests, options))
    }

    fn can_start(&self) -> bool {
        self.phase == AppPhase::Idle
            && self.queue.iter().any(|item| item.enabled)
            && self
                .queue
                .iter()
                .filter(|item| item.enabled)
                .all(|item| item.status.can_start())
    }

    fn start_blocker(&self) -> String {
        if self.phase != AppPhase::Idle {
            "a batch is already active".to_owned()
        } else if !self.queue.iter().any(|item| item.enabled) {
            "enable at least one queue item before starting".to_owned()
        } else {
            "wait for every enabled file to finish probing or resolve its error".to_owned()
        }
    }

    fn rebuild_plans(&mut self) {
        for index in 0..self.queue.len() {
            if self.queue[index].media.is_some() {
                self.queue[index].output = default_output(
                    &self.queue[index].input,
                    self.settings.output_directory.as_deref(),
                );
                self.rebuild_plan(index);
            }
        }
    }

    fn rebuild_plan(&mut self, index: usize) {
        let Some(media) = self.queue[index].media.as_ref() else {
            return;
        };
        let target = match self.settings.target() {
            Ok(target) => target,
            Err(error) => {
                self.queue[index].status = QueueStatus::Failed;
                self.queue[index].error = Some(error);
                return;
            }
        };
        let mode = match self.settings.output_mode() {
            Ok(mode) => mode,
            Err(error) => {
                self.queue[index].status = QueueStatus::Failed;
                self.queue[index].error = Some(error);
                return;
            }
        };
        let action = if self.settings.action_remux {
            RequestedAction::RemuxOnly {
                selection: sonicmux_core::AudioSelector::FirstCompatible,
            }
        } else {
            RequestedAction::Convert
        };
        let policy = PlanningPolicy::new(
            self.settings.policy(),
            target,
            mode,
            action,
            self.queue[index].output.clone(),
        );
        match sonicmux_core::build(media, &policy) {
            Ok(plan) => {
                self.queue[index].status = match plan {
                    PlanOutcome::Execute(_) => QueueStatus::Ready,
                    PlanOutcome::Skip(_) => QueueStatus::Compatible,
                    _ => QueueStatus::Ready,
                };
                self.queue[index].plan = Some(plan);
                self.queue[index].error = None;
            }
            Err(error) => {
                self.queue[index].status = QueueStatus::Failed;
                self.queue[index].plan = None;
                self.queue[index].error = Some(error.to_string());
            }
        }
    }

    fn move_selection(&mut self, delta: i32) {
        if self.screen == Screen::Settings {
            self.settings.selected_field = move_bounded(
                self.settings.selected_field,
                UiSettings::field_count(),
                delta,
            );
            return;
        }
        if self.queue.is_empty() {
            self.selected = None;
            return;
        }
        self.selected = Some(move_bounded(
            self.selected.unwrap_or(0),
            self.queue.len(),
            delta,
        ));
    }

    fn select_first(&mut self) {
        if self.screen == Screen::Settings {
            self.settings.selected_field = 0;
        } else if !self.queue.is_empty() {
            self.selected = Some(0);
        }
    }

    fn select_last(&mut self) {
        if self.screen == Screen::Settings {
            self.settings.selected_field = UiSettings::field_count().saturating_sub(1);
        } else if !self.queue.is_empty() {
            self.selected = Some(self.queue.len().saturating_sub(1));
        }
    }

    fn remove_selected(&mut self) {
        let Some(index) = self.selected else {
            return;
        };
        if index < self.queue.len() {
            self.queue.remove(index);
        }
        self.selected = if self.queue.is_empty() {
            None
        } else {
            Some(index.min(self.queue.len().saturating_sub(1)))
        };
    }

    fn retry_terminal(&mut self) -> Vec<Effect> {
        let mut effects = Vec::new();
        for item in &mut self.queue {
            if matches!(item.status, QueueStatus::Failed | QueueStatus::Cancelled) {
                item.enabled = true;
                if item.media.is_some() {
                    item.status = QueueStatus::Ready;
                    item.error = None;
                } else {
                    item.status = QueueStatus::Probing;
                    item.error = None;
                    effects.push(Effect::Probe(item.id, item.input.clone()));
                }
            }
        }
        self.rebuild_plans();
        effects
    }

    fn log(&mut self, message: String) {
        if self.logs.len() == LOG_CAPACITY {
            self.logs.pop_front();
        }
        self.logs.push_back(message);
    }
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

fn bitrate_values(codec: &str) -> &'static [&'static str] {
    match codec {
        "ac3" => &["384k", "448k", "640k"],
        "eac3" => &["384k", "640k", "1024k"],
        _ => &["192k", "256k", "384k"],
    }
}

fn default_bitrate(codec: &str) -> &'static str {
    match codec {
        "ac3" | "eac3" => "640k",
        _ => "384k",
    }
}

fn cycle_value(current: &str, values: &[&str], direction: i32) -> String {
    let current = values
        .iter()
        .position(|value| *value == current)
        .unwrap_or(0);
    values[cycle_index(current, values.len(), direction)].to_owned()
}

fn cycle_index(current: usize, length: usize, direction: i32) -> usize {
    if length == 0 {
        return 0;
    }
    if direction < 0 {
        current.checked_sub(1).unwrap_or(length - 1)
    } else {
        (current + 1) % length
    }
}

fn move_bounded(current: usize, length: usize, delta: i32) -> usize {
    if length == 0 {
        return 0;
    }
    if delta < 0 {
        current.saturating_sub(1)
    } else {
        current.saturating_add(1).min(length - 1)
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

fn queue_status(status: FileStatus) -> QueueStatus {
    match status {
        FileStatus::Queued => QueueStatus::Queued,
        FileStatus::Preparing => QueueStatus::Preparing,
        FileStatus::Ready => QueueStatus::Ready,
        FileStatus::Running => QueueStatus::Running,
        FileStatus::Succeeded => QueueStatus::Succeeded,
        FileStatus::Skipped => QueueStatus::Skipped,
        FileStatus::Planned => QueueStatus::Planned,
        FileStatus::Failed => QueueStatus::Failed,
        FileStatus::Cancelled => QueueStatus::Cancelled,
    }
}

fn previous_boundary(value: &str, cursor: usize) -> usize {
    value[..cursor]
        .char_indices()
        .next_back()
        .map_or(0, |(index, _)| index)
}

fn next_boundary(value: &str, cursor: usize) -> usize {
    value[cursor..]
        .char_indices()
        .nth(1)
        .map_or(value.len(), |(index, _)| cursor + index)
}

fn insert_char(value: &mut String, cursor: &mut usize, character: char) {
    value.insert(*cursor, character);
    *cursor = cursor.saturating_add(character.len_utf8());
}

fn remove_previous_char(value: &mut String, cursor: &mut usize) {
    if *cursor == 0 {
        return;
    }
    let previous = previous_boundary(value, *cursor);
    value.drain(previous..*cursor);
    *cursor = previous;
}

fn remove_next_char(value: &mut String, cursor: usize) {
    if cursor >= value.len() {
        return;
    }
    let next = next_boundary(value, cursor);
    value.drain(cursor..next);
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests {
    use std::{ffi::OsString, path::Path, sync::Arc};

    use async_trait::async_trait;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use sonicmux_backend::{
        BackendError, BackendExecution, BackendReport, MediaBackend, ProgressEvent,
    };
    use sonicmux_runtime::{DefaultConfig, PartialConfig, merge_config};
    use tempfile::tempdir;
    use tokio::sync::mpsc;
    use tokio_util::sync::CancellationToken;

    use super::*;

    fn model() -> Model {
        let config = merge_config(
            DefaultConfig::default(),
            PartialConfig::default(),
            PartialConfig::default(),
            PartialConfig::default(),
        )
        .expect("default config is valid");
        let settings =
            UiSettings::from_config(&config, false, None).expect("default settings are valid");
        Model::new(
            settings,
            DiscoveryRequest {
                roots: Vec::new(),
                recursive: false,
                follow_links: false,
                includes: Vec::new(),
                excludes: Vec::new(),
            },
            true,
        )
    }

    fn key(code: KeyCode) -> Msg {
        Msg::Input(KeyEvent::new(code, KeyModifiers::NONE))
    }

    #[test]
    fn navigation_and_overlays_are_keyboard_complete() {
        let mut model = model();
        model.update(key(KeyCode::Char('4')));
        assert_eq!(model.screen, Screen::Settings);
        model.update(key(KeyCode::Char('?')));
        assert!(matches!(model.overlay, Some(Overlay::Help)));
        model.update(key(KeyCode::Esc));
        assert!(model.overlay.is_none());
        model.update(key(KeyCode::Char('a')));
        model.update(key(KeyCode::Char('ф')));
        model.update(key(KeyCode::Backspace));
        assert!(matches!(
            model.overlay,
            Some(Overlay::PathEditor { ref value, cursor: 0 }) if value.is_empty()
        ));
    }

    #[test]
    fn discovery_deduplicates_and_selection_stays_bounded() {
        let mut model = model();
        let path = PathBuf::from("movie.mkv");
        let effects = model.update(Msg::InputsDiscovered(Ok(vec![path.clone(), path])));
        assert_eq!(model.queue.len(), 1);
        assert_eq!(effects.len(), 1);
        model.update(key(KeyCode::Char('G')));
        model.update(key(KeyCode::Down));
        assert_eq!(model.selected, Some(0));
        model.update(key(KeyCode::Char('d')));
        assert!(model.queue.is_empty());
        assert_eq!(model.selected, None);
    }

    #[test]
    fn active_batch_requires_orderly_cancellation() {
        let mut model = model();
        model.phase = AppPhase::Running;
        let effects = model.update(key(KeyCode::Char('q')));
        assert!(effects.is_empty());
        assert!(matches!(model.overlay, Some(Overlay::ConfirmCancel)));
        let effects = model.update(key(KeyCode::Char('y')));
        assert!(matches!(effects.as_slice(), [Effect::CancelBatch]));
        assert_eq!(model.phase, AppPhase::Cancelling);
        assert!(!model.should_quit);
    }

    #[test]
    fn background_failure_is_visible_and_cancels_active_work() {
        let mut model = model();
        model.phase = AppPhase::Running;
        let effects = model.update(Msg::TaskFailed("input reader stopped".to_owned()));
        assert!(matches!(effects.as_slice(), [Effect::CancelBatch]));
        assert_eq!(model.phase, AppPhase::Cancelling);
        assert!(matches!(model.overlay, Some(Overlay::Notice(_))));
        assert!(
            model
                .logs
                .back()
                .is_some_and(|line| line.contains("input reader"))
        );
    }

    #[test]
    fn unicode_editor_uses_character_boundaries() {
        let mut value = "aяb".to_owned();
        let mut cursor = "aя".len();
        remove_previous_char(&mut value, &mut cursor);
        assert_eq!(value, "ab");
        assert_eq!(cursor, 1);
        insert_char(&mut value, &mut cursor, '界');
        assert_eq!(value, "a界b");
    }

    #[test]
    fn startup_effect_preserves_non_unicode_roots() {
        let mut model = model();
        model.discovery_template.roots = vec![OsString::from("movie.mkv")];
        assert!(matches!(model.startup_effect(), Some(Effect::Discover(_))));
    }

    #[derive(Clone)]
    struct ProbeOnlyBackend {
        media: MediaInfo,
    }

    #[async_trait]
    impl MediaBackend for ProbeOnlyBackend {
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
                source: Box::new(std::io::Error::other("dry-run must not execute")),
            })
        }
    }

    #[tokio::test]
    async fn dry_run_reconciles_authoritative_scheduler_report() {
        let directory = tempdir().expect("temporary directory is created");
        let input = directory.path().join("movie.mkv");
        let media = sonicmux_ffmpeg::parse_probe_output(
            input.clone(),
            include_bytes!("../../ffmpeg/tests/fixtures/mixed.json"),
        )
        .expect("fixture parses");
        let mut model = model();
        model.settings.dry_run = true;
        let effects = model.update(Msg::InputsDiscovered(Ok(vec![input.clone()])));
        let id = match effects.as_slice() {
            [Effect::Probe(id, path)] if path == &input => *id,
            other => panic!("unexpected effects: {other:?}"),
        };
        model.update(Msg::ProbeFinished(id, Ok(media.clone())));
        assert_eq!(model.queue[0].status, QueueStatus::Ready);

        let request = model.build_batch().expect("ready queue builds a batch");
        let runtime = sonicmux_runtime::Runtime::new(Arc::new(ProbeOnlyBackend { media }));
        let report = runtime
            .start_batch(request, CancellationToken::new())
            .wait()
            .await
            .expect("dry-run scheduler completes");
        model.update(Msg::BatchFinished(Ok(report)));

        assert_eq!(model.phase, AppPhase::Idle);
        assert_eq!(model.queue[0].status, QueueStatus::Planned);
        assert!(model.queue[0].error.is_none());
    }
}
