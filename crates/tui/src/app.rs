use std::{
    io::{self, IsTerminal},
    sync::Arc,
    time::Duration,
};

use color_eyre::eyre::{Result, WrapErr, eyre};
use sonicmux_ffmpeg::{FfmpegCliBackend, resolve_toolchain};
use sonicmux_runtime::{
    BatchEvent, DiscoveryRequest, PartialConfig, Runtime, discover, load_effective_config,
    observability::{ObservabilityGuard, ObservabilityOptions, init_tracing_with},
    select_config_path,
};
use tokio::{
    sync::{Semaphore, broadcast, mpsc},
    task::JoinSet,
    time::{MissedTickBehavior, interval},
};
use tokio_util::sync::CancellationToken;

use crate::{
    args::TuiArgs,
    input::InputReader,
    model::{AppPhase, Effect, Model, Msg, Overlay, UiSettings},
    terminal::TerminalSession,
    ui,
};

/// Running SonicMux terminal application and its shared runtime dependencies.
pub struct App {
    runtime: Runtime,
    model: Model,
    _observability: ObservabilityGuard,
    root_cancel: CancellationToken,
    probe_limit: Arc<Semaphore>,
    tasks: JoinSet<()>,
    active_batch_cancel: Option<CancellationToken>,
}

impl App {
    /// Loads configuration, resolves FFmpeg, and creates a terminal application.
    pub fn new(arguments: TuiArgs) -> Result<Self> {
        if let Some(directory) = &arguments.output_dir {
            if !directory.is_dir() {
                return Err(eyre!(
                    "output directory does not exist: {}",
                    directory.display()
                ));
            }
        }
        let config_path = select_config_path(arguments.config.clone())?;
        let mut overrides = PartialConfig::default();
        overrides.ffmpeg_path = arguments.ffmpeg_path.clone();
        overrides.output_directory = arguments.output_dir.clone();
        overrides.log_file = arguments.log_file.clone();
        overrides.color = arguments.no_color.then(|| "never".to_owned());
        let config = load_effective_config(&config_path, overrides)?;
        let observability = init_tracing_with(ObservabilityOptions {
            filter: std::env::var("RUST_LOG").unwrap_or_else(|_| "sonicmux=info,warn".to_owned()),
            console: false,
            file: config.log_file.as_ref().map(|value| value.value().clone()),
        })?;
        let explicit = config
            .ffmpeg_path
            .as_ref()
            .map(|value| value.value().as_path());
        let toolchain = resolve_toolchain(explicit)
            .wrap_err("FFmpeg and FFprobe are required by sonicmux-tui")?;
        let runtime = Runtime::new(Arc::new(FfmpegCliBackend::new(toolchain)));
        let output_directory = config
            .output_directory
            .as_ref()
            .map(|value| value.value().clone());
        let settings = UiSettings::from_config(&config, arguments.dry_run, output_directory)?;
        let color = !arguments.no_color
            && std::env::var_os("NO_COLOR").is_none()
            && match config.color.value().as_str() {
                "always" => true,
                "never" => false,
                _ => io::stdout().is_terminal(),
            };
        let discovery = DiscoveryRequest {
            roots: arguments.inputs,
            recursive: arguments.recursive,
            follow_links: arguments.follow_links,
            includes: arguments.include,
            excludes: arguments.exclude,
        };
        let probe_jobs = settings.jobs.clamp(1, 4);
        Ok(Self {
            runtime,
            model: Model::new(settings, discovery, color),
            _observability: observability,
            root_cancel: CancellationToken::new(),
            probe_limit: Arc::new(Semaphore::new(probe_jobs)),
            tasks: JoinSet::new(),
            active_batch_cancel: None,
        })
    }

    /// Runs the full-screen application and restores the terminal on return.
    pub async fn run(mut self) -> Result<()> {
        let mut session = TerminalSession::init()?;
        let result = self.run_loop(session.terminal()).await;
        session.finish(result)
    }

    async fn run_loop(&mut self, terminal: &mut ratatui::DefaultTerminal) -> Result<()> {
        let (sender, mut receiver) = mpsc::channel(256);
        let input = InputReader::spawn(sender.clone());
        if let Some(effect) = self.model.startup_effect() {
            self.apply_effect(effect, &sender);
        }
        let mut redraw = interval(Duration::from_millis(33));
        redraw.set_missed_tick_behavior(MissedTickBehavior::Skip);
        let mut dirty = true;
        while !self.model.should_quit {
            tokio::select! {
                _ = redraw.tick() => {
                    let _effects = self.model.update(Msg::Tick);
                    if dirty {
                        terminal.draw(|frame| ui::render(frame, &self.model))
                            .wrap_err("failed to render the terminal interface")?;
                        dirty = false;
                    }
                }
                message = receiver.recv() => {
                    let Some(message) = message else { break; };
                    let batch_finished = matches!(message, Msg::BatchFinished(_));
                    let effects = self.model.update(message);
                    if batch_finished {
                        self.active_batch_cancel = None;
                    }
                    for effect in effects {
                        self.apply_effect(effect, &sender);
                    }
                    dirty = true;
                }
            }
        }
        if self.model.phase != AppPhase::Idle {
            if let Some(cancel) = &self.active_batch_cancel {
                cancel.cancel();
            }
        }
        self.root_cancel.cancel();
        input.stop();
        while self.tasks.join_next().await.is_some() {}
        Ok(())
    }

    fn apply_effect(&mut self, effect: Effect, sender: &mpsc::Sender<Msg>) {
        match effect {
            Effect::Discover(request) => {
                let sender = sender.clone();
                self.tasks.spawn(async move {
                    let result = discover(request).await;
                    let _ignored = sender.send(Msg::InputsDiscovered(result)).await;
                });
            }
            Effect::Probe(id, path) => {
                let sender = sender.clone();
                let runtime = self.runtime.clone();
                let semaphore = Arc::clone(&self.probe_limit);
                let cancel = self.root_cancel.child_token();
                self.tasks.spawn(async move {
                    let permit = semaphore.acquire_owned().await;
                    if permit.is_err() {
                        return;
                    }
                    let result = runtime.probe(&path, cancel).await;
                    let _ignored = sender.send(Msg::ProbeFinished(id, result)).await;
                });
            }
            Effect::StartBatch => match self.model.build_batch() {
                Ok(request) => self.start_batch(request, sender),
                Err(error) => {
                    self.model.phase = AppPhase::Idle;
                    self.model.overlay = Some(Overlay::Notice(error));
                }
            },
            Effect::CancelBatch => {
                if let Some(cancel) = &self.active_batch_cancel {
                    cancel.cancel();
                }
            }
        }
    }

    fn start_batch(&mut self, request: sonicmux_runtime::BatchRequest, sender: &mpsc::Sender<Msg>) {
        let cancel = self.root_cancel.child_token();
        let handle = self.runtime.start_batch(request, cancel.clone());
        self.active_batch_cancel = Some(cancel);
        let (mut snapshots, mut events, waiter) = handle.into_parts();

        let snapshot_sender = sender.clone();
        self.tasks.spawn(async move {
            let initial = snapshots.borrow().clone();
            if snapshot_sender
                .send(Msg::BatchSnapshot(initial))
                .await
                .is_err()
            {
                return;
            }
            while snapshots.changed().await.is_ok() {
                let snapshot = snapshots.borrow().clone();
                if snapshot_sender
                    .send(Msg::BatchSnapshot(snapshot))
                    .await
                    .is_err()
                {
                    break;
                }
            }
        });

        let event_sender = sender.clone();
        self.tasks.spawn(async move {
            loop {
                match events.recv().await {
                    Ok(BatchEvent::FileProgress { .. }) => {}
                    Ok(event) => {
                        if event_sender.send(Msg::BatchEvent(event)).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(count)) => {
                        if event_sender.send(Msg::EventsLagged(count)).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });

        let waiter_sender = sender.clone();
        self.tasks.spawn(async move {
            let result = waiter.wait().await;
            let _ignored = waiter_sender.send(Msg::BatchFinished(result)).await;
        });
    }
}
