#![doc = "Command-line entry point for SonicMux."]
#![forbid(unsafe_code)]

mod args;
mod dto;
mod human;

use std::{
    ffi::OsString,
    fs::OpenOptions,
    io::{self, IsTerminal as _, Write as _},
    num::NonZeroUsize,
    path::{Path, PathBuf},
    process::ExitCode,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use args::{
    Cli, Command, CompletionShell, ConfigCommand, ConvertArgs, DoctorArgs, PresetsCommand,
    ProbeArgs, ScanArgs,
};
use clap::{CommandFactory as _, Parser as _, error::ErrorKind};
use dto::{DoctorDto, PathDto, PlanDto, ProbeDto, ProgressDto};
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use serde::Serialize;
use serde_json::{Value, json};
use sonicmux_backend::{CapabilityRequest, MediaCapability};
use sonicmux_core::{
    AudioSelector, Compatibility, JobPlan, PlanOutcome, PlanningPolicy, RequestedAction,
    StreamIndex,
};
use sonicmux_ffmpeg::{FfmpegCliBackend, resolve_toolchain};
use sonicmux_runtime::{
    ActionRequest, AudioSelectionRequest, BatchEvent, BatchReport, BatchRequest, BatchSnapshot,
    CancellationReason, ConfigError, ConfigPath, DefaultConfig, DiscoveryRequest, DryRunReport,
    EffectiveConfig, ExistingOutputOutcome, FailurePolicy, FileOutcome, FileRequest, FileStatus,
    PartialConfig, Runtime, RuntimeError, SchedulerOptions, discover, initialize_config,
    load_effective_config, load_file, merge_config, select_config_path,
};
use tokio::sync::{broadcast, watch};
use tokio_util::sync::CancellationToken;

#[derive(Debug)]
struct CliFailure {
    code: u8,
    message: String,
}

impl CliFailure {
    fn new(code: u8, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct OutputMode {
    json: bool,
    json_progress: bool,
    quiet: bool,
    color: bool,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => {
            let code = if matches!(
                error.kind(),
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
            ) {
                0
            } else {
                2
            };
            let _printed = error.print();
            return ExitCode::from(code);
        }
    };
    let output = OutputMode {
        json: cli.json,
        json_progress: cli.json_progress,
        quiet: cli.quiet,
        color: false,
    };
    let command_name = command_name(&cli.command);
    match run(cli).await {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            emit_failure(output, command_name, &error);
            ExitCode::from(error.code)
        }
    }
}

async fn run(cli: Cli) -> Result<u8, CliFailure> {
    if matches!(cli.command, Command::Completions(_) | Command::Man(_)) {
        if cli.json || cli.json_progress {
            return Err(CliFailure::new(
                2,
                "completions and man emit raw artifacts and cannot use JSON modes",
            ));
        }
        return generate_artifact(&cli.command);
    }
    if cli.json_progress
        && !matches!(
            cli.command,
            Command::Probe(_) | Command::Scan(_) | Command::Convert(_)
        )
    {
        return Err(CliFailure::new(
            2,
            "--json-progress is supported only by probe, scan, and convert",
        ));
    }

    let config_path = select_config_path(cli.config.clone()).map_err(config_failure)?;
    if let Command::Config(arguments) = &cli.command {
        let output = OutputMode {
            json: cli.json,
            json_progress: cli.json_progress,
            quiet: cli.quiet,
            color: cli
                .color
                .is_some_and(|choice| matches!(choice, args::ColorChoice::Always)),
        };
        match &arguments.command {
            ConfigCommand::Path => {
                if output.json {
                    emit_result(
                        "config",
                        "success",
                        json!({"path": PathDto::new(&config_path.path)}),
                    )?;
                } else if !output.quiet {
                    write_human(&config_path.path.display().to_string(), output.color)?;
                }
                return Ok(0);
            }
            ConfigCommand::Init => {
                initialize_config(&config_path).map_err(config_failure)?;
                if output.json {
                    emit_result(
                        "config",
                        "success",
                        json!({"created": PathDto::new(&config_path.path)}),
                    )?;
                } else if !output.quiet {
                    write_human(
                        &format!("Created {}", config_path.path.display()),
                        output.color,
                    )?;
                }
                return Ok(0);
            }
            ConfigCommand::Validate => {
                let _validated = load_file(&ConfigPath {
                    path: config_path.path.clone(),
                    required: true,
                })
                .map_err(config_failure)?;
                if output.json {
                    emit_result(
                        "config",
                        "success",
                        json!({"valid": true, "path": PathDto::new(&config_path.path)}),
                    )?;
                } else if !output.quiet {
                    write_human(
                        &format!("Configuration is valid: {}", config_path.path.display()),
                        output.color,
                    )?;
                }
                return Ok(0);
            }
            ConfigCommand::Show { .. } => {}
        }
    }
    let overrides = command_overrides(&cli);
    let config = load_effective_config(&config_path, overrides).map_err(config_failure)?;
    let filter = if std::env::var_os("RUST_LOG").is_some() {
        std::env::var("RUST_LOG").map_err(|_| CliFailure::new(2, "RUST_LOG must be Unicode"))?
    } else {
        match cli.verbose {
            0 => "warn".to_owned(),
            1 => "sonicmux=info,warn".to_owned(),
            _ => "sonicmux=debug,info".to_owned(),
        }
    };
    let _tracing = sonicmux_runtime::observability::init_tracing_with(
        sonicmux_runtime::observability::ObservabilityOptions {
            filter,
            console: !cli.quiet && !cli.json && !cli.json_progress,
            file: config.log_file.as_ref().map(|value| value.value().clone()),
        },
    )
    .map_err(|error| CliFailure::new(2, error.to_string()))?;
    let output = OutputMode {
        json: cli.json,
        json_progress: cli.json_progress,
        quiet: cli.quiet,
        color: color_enabled(config.color.value()),
    };

    match &cli.command {
        Command::Config(arguments) => {
            config_command(&arguments.command, &config_path, &config, output)
        }
        Command::Presets(arguments) => presets_command(&arguments.command, &config, output),
        Command::Probe(arguments) => {
            let runtime = runtime(&config)?;
            probe_command(arguments, &runtime, output).await
        }
        Command::Scan(arguments) => {
            let runtime = runtime(&config)?;
            scan_command(arguments, &runtime, &config, output).await
        }
        Command::Convert(arguments) => {
            let runtime = runtime(&config)?;
            convert_command(arguments, &runtime, &config, output).await
        }
        Command::Doctor(arguments) => {
            let runtime = runtime(&config)?;
            doctor_command(arguments, &runtime, &config, output).await
        }
        Command::Completions(_) | Command::Man(_) => unreachable!("handled before config"),
    }
}

fn command_overrides(cli: &Cli) -> PartialConfig {
    let audio = match &cli.command {
        Command::Convert(arguments) => Some(&arguments.audio),
        Command::Scan(arguments) => Some(&arguments.audio),
        _ => None,
    };
    let doctor = match &cli.command {
        Command::Doctor(arguments) => Some(arguments),
        _ => None,
    };
    let mut config = PartialConfig::default();
    config.profile = audio
        .and_then(|value| value.profile.clone())
        .or_else(|| doctor.and_then(|value| value.profile.clone()));
    config.codec = audio
        .and_then(|value| value.codec.clone())
        .or_else(|| doctor.and_then(|value| value.codec.clone()));
    config.bitrate = audio.and_then(|value| value.bitrate.clone());
    config.channels = audio.and_then(|value| value.channels.clone());
    config.mode = audio.and_then(|value| value.mode.clone());
    config.ffmpeg_path = cli.ffmpeg_path.clone();
    config.color = cli.color.map(|value| value.as_str().to_owned());
    config.log_file = cli.log_file.clone();
    if let Command::Convert(arguments) = &cli.command {
        config.jobs = arguments.jobs;
        config.storage_profile = arguments
            .storage_profile
            .map(|profile| profile.as_str().to_owned());
    }
    config
}

fn runtime(config: &EffectiveConfig) -> Result<Runtime, CliFailure> {
    let explicit = config
        .ffmpeg_path
        .as_ref()
        .map(|value| value.value().as_path());
    let paths =
        resolve_toolchain(explicit).map_err(|error| CliFailure::new(3, error.to_string()))?;
    Ok(Runtime::new(Arc::new(FfmpegCliBackend::new(paths))))
}

async fn probe_command(
    arguments: &ProbeArgs,
    runtime: &Runtime,
    output: OutputMode,
) -> Result<u8, CliFailure> {
    let cancel = cancellation_token();
    let sequence = Arc::new(AtomicU64::new(0));
    if output.json_progress {
        emit_event(&sequence, "batch_started", json!({"command": "probe"}))?;
    }
    let mut results = Vec::new();
    let mut failures = 0_usize;
    for input in &arguments.inputs {
        let path = PathBuf::from(input);
        if output.json_progress {
            emit_event(
                &sequence,
                "file_started",
                json!({"path": PathDto::new(&path)}),
            )?;
        }
        if let Err(error) = validate_probe_input(&path) {
            failures += 1;
            results.push(error_value(&path, &error));
            if output.json_progress {
                emit_event(&sequence, "file_failed", error_value(&path, &error))?;
            } else if !output.json {
                eprintln!("error: {}: {}", path.display(), error.message);
            }
            continue;
        }
        match runtime.probe(&path, cancel.clone()).await {
            Ok(media) => {
                let dto = ProbeDto::from(&media);
                results.push(json!({"status": "success", "media": dto}));
                if output.json_progress {
                    emit_event(&sequence, "file_succeeded", json!({"media": dto}))?;
                } else if !output.json && !output.quiet {
                    write_human(&human::probe(&media, arguments.compact), output.color)?;
                }
            }
            Err(error) => {
                if is_cancelled(&error) {
                    let failure = CliFailure::new(130, error.to_string());
                    if output.json_progress {
                        emit_event(
                            &sequence,
                            "batch_cancelled",
                            json!({"message": failure.message}),
                        )?;
                    }
                    return Err(failure);
                }
                failures += 1;
                let failure = runtime_failure(error, 4);
                results.push(error_value(&path, &failure));
                if output.json_progress {
                    emit_event(&sequence, "file_failed", error_value(&path, &failure))?;
                } else if !output.json {
                    eprintln!("error: {}: {}", path.display(), failure.message);
                }
            }
        }
    }
    let code = batch_code(arguments.inputs.len(), failures, 4);
    if output.json {
        emit_result("probe", status(code), json!({"files": results}))?;
    } else if output.json_progress {
        emit_event(
            &sequence,
            "batch_finished",
            json!({"status": status(code), "exit_code": code}),
        )?;
    }
    Ok(code)
}

async fn scan_command(
    arguments: &ScanArgs,
    runtime: &Runtime,
    config: &EffectiveConfig,
    output: OutputMode,
) -> Result<u8, CliFailure> {
    let files = discover(discovery_request(&arguments.inputs, &arguments.discovery))
        .await
        .map_err(|error| CliFailure::new(4, error.to_string()))?;
    let cancel = cancellation_token();
    let sequence = Arc::new(AtomicU64::new(0));
    if output.json_progress {
        emit_event(
            &sequence,
            "batch_started",
            json!({"command": "scan", "files": files.len()}),
        )?;
    }
    let mut results = Vec::new();
    let mut failures = 0;
    for path in &files {
        if output.json_progress {
            emit_event(
                &sequence,
                "file_started",
                json!({"path": PathDto::new(path)}),
            )?;
        }
        match analyze(
            runtime,
            config,
            path,
            None,
            false,
            "first-compatible",
            cancel.clone(),
        )
        .await
        {
            Ok(Analyzed::Skip) => {
                if !arguments.needs_action {
                    results.push(json!({"path": PathDto::new(path), "status": "compatible"}));
                    if !output.json && !output.json_progress && !output.quiet {
                        write_human(&format!("{}: compatible", path.display()), output.color)?;
                    }
                }
                if output.json_progress {
                    emit_event(
                        &sequence,
                        "file_succeeded",
                        json!({"path": PathDto::new(path), "action": "none"}),
                    )?;
                }
            }
            Ok(Analyzed::Plan {
                plan,
                remux_available,
            }) => {
                let scan_status = if remux_available {
                    "remux-available"
                } else {
                    "transcode"
                };
                let dto = PlanDto::from(plan.as_ref());
                results.push(json!({"status": scan_status, "plan": dto}));
                if output.json_progress {
                    emit_event(
                        &sequence,
                        "file_succeeded",
                        json!({"status": scan_status, "plan": dto}),
                    )?;
                } else if !output.json && !output.quiet {
                    write_human(
                        &format!("{scan_status}: {}", human::plan(&plan)),
                        output.color,
                    )?;
                }
            }
            Err(error) => {
                if error.code == 130 {
                    return Err(error);
                }
                failures += 1;
                results.push(error_value(path, &error));
                if output.json_progress {
                    emit_event(&sequence, "file_failed", error_value(path, &error))?;
                } else if !output.json {
                    eprintln!("error: {}: {}", path.display(), error.message);
                }
            }
        }
    }
    let code = batch_code(files.len(), failures, 4);
    if output.json {
        emit_result("scan", status(code), json!({"files": results}))?;
    } else if output.json_progress {
        emit_event(
            &sequence,
            "batch_finished",
            json!({"status": status(code), "exit_code": code}),
        )?;
    }
    Ok(code)
}

async fn convert_command(
    arguments: &ConvertArgs,
    runtime: &Runtime,
    config: &EffectiveConfig,
    output: OutputMode,
) -> Result<u8, CliFailure> {
    if arguments.remux_only
        && (arguments.audio.codec.is_some()
            || arguments.audio.bitrate.is_some()
            || arguments.audio.channels.is_some()
            || arguments.audio.mode.is_some())
    {
        return Err(CliFailure::new(
            2,
            "--remux-only conflicts with codec, bitrate, channels, and mode overrides",
        ));
    }
    let files = discover(discovery_request(&arguments.inputs, &arguments.discovery))
        .await
        .map_err(|error| CliFailure::new(4, error.to_string()))?;
    if arguments.output.is_some() && files.len() != 1 {
        return Err(CliFailure::new(
            2,
            "--output requires exactly one discovered input file",
        ));
    }
    if let Some(directory) = arguments
        .output_dir
        .as_ref()
        .or_else(|| config.output_directory.as_ref().map(|value| value.value()))
    {
        if !directory.is_dir() {
            return Err(CliFailure::new(
                2,
                format!("output directory does not exist: {}", directory.display()),
            ));
        }
    }
    let compatibility = Arc::new(config.compatibility_policy().map_err(config_failure)?);
    let target = config.audio_target().map_err(config_failure)?;
    let output_mode = config.output_mode().map_err(config_failure)?;
    let action = action_request(arguments);
    let mut requests = Vec::with_capacity(files.len());
    for path in &files {
        requests.push(FileRequest::new(
            path.clone(),
            resolve_output(path, arguments, config)?,
            Arc::clone(&compatibility),
            target.clone(),
            output_mode,
            action.clone(),
        ));
    }
    let jobs = NonZeroUsize::new(*config.jobs.value())
        .ok_or_else(|| CliFailure::new(2, "scheduler jobs must be greater than zero"))?;
    let failure_policy = if arguments.fail_fast {
        FailurePolicy::FailFast
    } else {
        FailurePolicy::Continue
    };
    let options = SchedulerOptions::new(jobs, failure_policy, arguments.dry_run)
        .map_err(|error| CliFailure::new(2, error.to_string()))?;
    let cancel = cancellation_token();
    let sequence = Arc::new(AtomicU64::new(0));
    let handle = runtime.start_batch(BatchRequest::new(requests, options), cancel.clone());
    let (snapshots, events, waiter) = handle.into_parts();
    let render_sequence = Arc::clone(&sequence);
    let render_cancel = cancel.clone();
    let storage_profile = config.storage_profile.value().clone();
    let render_task = tokio::spawn(async move {
        let result = render_batch_events(
            events,
            snapshots,
            output,
            render_sequence,
            &storage_profile,
        )
        .await;
        if result.is_err() {
            render_cancel.cancel();
        }
        result
    });
    let report = waiter
        .wait()
        .await
        .map_err(|error| CliFailure::new(6, error.to_string()))?;
    render_task
        .await
        .map_err(|error| CliFailure::new(6, format!("progress renderer failed: {error}")))??;
    finish_convert_report(&report, output, &sequence)
}

fn action_request(arguments: &ConvertArgs) -> ActionRequest {
    if !arguments.remux_only {
        return ActionRequest::Convert;
    }
    if arguments.default_audio == "first-compatible" {
        return ActionRequest::RemuxOnly(AudioSelectionRequest::FirstCompatible);
    }
    match arguments.default_audio.parse::<u32>() {
        Ok(index) => {
            ActionRequest::RemuxOnly(AudioSelectionRequest::StreamIndex(StreamIndex::new(index)))
        }
        Err(_) => ActionRequest::RemuxOnly(AudioSelectionRequest::Language(
            arguments.default_audio.clone(),
        )),
    }
}

async fn render_batch_events(
    mut events: broadcast::Receiver<BatchEvent>,
    snapshots: watch::Receiver<Arc<BatchSnapshot>>,
    output: OutputMode,
    sequence: Arc<AtomicU64>,
    storage_profile: &str,
) -> Result<(), CliFailure> {
    let visible = !output.json
        && !output.json_progress
        && !output.quiet
        && io::stderr().is_terminal()
        && std::env::var_os("TERM").is_none_or(|value| value != "dumb");
    let multi = visible.then(MultiProgress::new);
    let mut preparation = None;
    let mut aggregate = None;
    let mut file_bars = std::collections::HashMap::new();
    loop {
        match events.recv().await {
            Ok(event) => {
                if output.json_progress {
                    emit_scheduler_event(&sequence, &event, storage_profile)?;
                }
                if let Some(multi) = &multi {
                    render_batch_snapshot(
                        multi,
                        &snapshots.borrow(),
                        &mut preparation,
                        &mut aggregate,
                        &mut file_bars,
                    );
                }
            }
            Err(broadcast::error::RecvError::Lagged(skipped)) => {
                if output.json_progress {
                    emit_event(
                        &sequence,
                        "progress_resync",
                        json!({
                            "skipped_events": skipped,
                            "snapshot": batch_snapshot_value(&snapshots.borrow()),
                        }),
                    )?;
                }
            }
            Err(broadcast::error::RecvError::Closed) => break,
        }
    }
    if let Some(multi) = multi {
        let _cleared = multi.clear();
    }
    Ok(())
}

fn emit_scheduler_event(
    sequence: &Arc<AtomicU64>,
    event: &BatchEvent,
    storage_profile: &str,
) -> Result<(), CliFailure> {
    match event {
        BatchEvent::BatchStarted {
            total,
            concurrency,
            failure_policy,
        } => emit_event(
            sequence,
            "batch_started",
            json!({
                "command": "convert",
                "files": total,
                "jobs": concurrency,
                "storage_profile": storage_profile,
                "failure_policy": failure_policy.as_str(),
            }),
        ),
        BatchEvent::PreparationStarted => emit_event(sequence, "preparation_started", json!({})),
        BatchEvent::FileStarted { id, path } => emit_event(
            sequence,
            "file_started",
            json!({"job_id": id.get(), "path": PathDto::new(path)}),
        ),
        BatchEvent::FilePrepared { id, path, status } => emit_event(
            sequence,
            "file_prepared",
            json!({
                "job_id": id.get(),
                "path": PathDto::new(path),
                "status": status.as_str(),
            }),
        ),
        BatchEvent::ExecutionStarted { ready } => {
            emit_event(sequence, "execution_started", json!({"ready": ready}))
        }
        BatchEvent::FileProgress { id, path, progress } => emit_event(
            sequence,
            "progress",
            json!({
                "job_id": id.get(),
                "path": PathDto::new(path),
                "progress": ProgressDto::from(progress),
            }),
        ),
        BatchEvent::FileFinished { id, path, status } => emit_event(
            sequence,
            "file_finished",
            json!({
                "job_id": id.get(),
                "path": PathDto::new(path),
                "status": status.as_str(),
            }),
        ),
        BatchEvent::BatchFinished | BatchEvent::BatchCancelled => Ok(()),
        _ => Ok(()),
    }
}

fn render_batch_snapshot(
    multi: &MultiProgress,
    snapshot: &BatchSnapshot,
    preparation: &mut Option<ProgressBar>,
    aggregate: &mut Option<ProgressBar>,
    file_bars: &mut std::collections::HashMap<usize, ProgressBar>,
) {
    if preparation.is_none() {
        let bar = multi.add(ProgressBar::new(snapshot.total() as u64));
        bar.set_style(
            ProgressStyle::with_template("{spinner:.green} Analyzing {pos}/{len}")
                .unwrap_or_else(|_| ProgressStyle::default_bar()),
        );
        *preparation = Some(bar);
    }
    if let Some(bar) = preparation {
        bar.set_position(snapshot.prepared() as u64);
        if snapshot.stage() != sonicmux_runtime::BatchStage::Preparing {
            bar.finish_and_clear();
        }
    }
    if snapshot.stage() == sonicmux_runtime::BatchStage::Executing {
        if aggregate.is_none() {
            let bar = multi.add(ProgressBar::new(1_000));
            bar.set_style(
                ProgressStyle::with_template(
                    "{spinner:.green} Overall {bar:36.cyan/blue} {pos:>4}/1000 {wide_msg}",
                )
                .unwrap_or_else(|_| ProgressStyle::default_bar()),
            );
            *aggregate = Some(bar);
        }
        if let Some(bar) = aggregate {
            if let Some(position) = snapshot.progress_milli() {
                bar.set_position(u64::from(position));
            }
            bar.set_message(snapshot.eta().map_or_else(
                || "ETA unknown".to_owned(),
                |eta| format!("ETA ~{}", human_duration(eta)),
            ));
        }
    }
    for file in snapshot.files() {
        if file.status() == FileStatus::Running && !file_bars.contains_key(&file.id().get()) {
            let bar = file.duration_us().map_or_else(
                || multi.add(ProgressBar::new_spinner()),
                |duration| multi.add(ProgressBar::new(duration)),
            );
            bar.set_style(
                ProgressStyle::with_template(
                    "{spinner:.green} {wide_msg} {bar:28.cyan/blue} {percent}%",
                )
                .unwrap_or_else(|_| ProgressStyle::default_bar()),
            );
            file_bars.insert(file.id().get(), bar);
        }
        if let Some(bar) = file_bars.get(&file.id().get()) {
            if let Some(position) = file.position_us() {
                bar.set_position(position);
            }
            let eta = file.eta().map_or_else(String::new, |value| {
                format!(" · ETA ~{}", human_duration(value))
            });
            bar.set_message(format!("{}{}", file.path().display(), eta));
        }
        if file.status().is_terminal() {
            if let Some(bar) = file_bars.remove(&file.id().get()) {
                bar.finish_and_clear();
            }
        }
    }
}

fn batch_snapshot_value(snapshot: &BatchSnapshot) -> Value {
    json!({
        "stage": snapshot.stage().as_str(),
        "total": snapshot.total(),
        "prepared": snapshot.prepared(),
        "active": snapshot.active(),
        "queued": snapshot.queued(),
        "completed": snapshot.completed(),
        "progress_milli": snapshot.progress_milli(),
        "eta_ms": snapshot.eta().map(|value| value.as_millis()),
    })
}

fn human_duration(duration: std::time::Duration) -> String {
    let seconds = duration.as_secs();
    let hours = seconds / 3_600;
    let minutes = (seconds % 3_600) / 60;
    let seconds = seconds % 60;
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

fn finish_convert_report(
    report: &BatchReport,
    output: OutputMode,
    sequence: &Arc<AtomicU64>,
) -> Result<u8, CliFailure> {
    let mut results = Vec::with_capacity(report.results().len());
    let mut failures = 0;
    let mut only_failure_code = 6;
    for result in report.results() {
        let path = result.input();
        match result.outcome() {
            FileOutcome::Succeeded(job) => {
                results.push(json!({
                    "path": PathDto::new(path),
                    "output": PathDto::new(job.output()),
                    "status": "success",
                    "elapsed_us": job.backend().elapsed().as_micros(),
                    "warnings": job.warnings(),
                }));
                emit_file_success(output, sequence, path, "converted")?;
            }
            FileOutcome::Skipped(reason) => {
                results.push(json!({
                    "path": PathDto::new(path),
                    "status": "skipped",
                    "reason": reason.as_str(),
                }));
                emit_file_success(output, sequence, path, reason.as_str())?;
            }
            FileOutcome::Planned(dry_run) => {
                let state = existing_state(dry_run.existing());
                let plan = PlanDto::from(dry_run.plan());
                results.push(json!({
                    "status": "dry-run",
                    "existing_output": state,
                    "plan": plan,
                }));
                if output.json_progress {
                    emit_event(
                        sequence,
                        "file_succeeded",
                        json!({
                            "job_id": result.id().get(),
                            "status": "dry-run",
                            "existing_output": state,
                            "plan": plan,
                        }),
                    )?;
                } else if !output.json && !output.quiet {
                    write_human(&dry_run_text(dry_run), output.color)?;
                }
            }
            FileOutcome::Failed(failure) => {
                failures += 1;
                only_failure_code = failure.stage().exit_code();
                let error = CliFailure::new(only_failure_code, failure.message());
                results.push(error_value(path, &error));
                emit_file_failure(output, sequence, path, &error)?;
            }
            FileOutcome::Cancelled(reason) => {
                results.push(json!({
                    "path": PathDto::new(path),
                    "status": "cancelled",
                    "reason": reason.as_str(),
                }));
                if output.json_progress {
                    emit_event(
                        sequence,
                        "file_cancelled",
                        json!({
                            "job_id": result.id().get(),
                            "path": PathDto::new(path),
                            "reason": reason.as_str(),
                        }),
                    )?;
                } else if !output.json && !output.quiet {
                    eprintln!("cancelled: {} ({})", path.display(), reason.as_str());
                }
            }
            _ => {}
        }
    }
    let code = if report.cancellation() == Some(CancellationReason::User) {
        130
    } else {
        batch_code(report.results().len(), failures, only_failure_code)
    };
    if output.json {
        emit_result("convert", status(code), json!({"files": results}))?;
    } else if output.json_progress {
        emit_event(
            sequence,
            if code == 130 {
                "batch_cancelled"
            } else {
                "batch_finished"
            },
            json!({"status": status(code), "exit_code": code}),
        )?;
    }
    Ok(code)
}

fn dry_run_text(report: &DryRunReport) -> String {
    format!(
        "dry-run: {}; existing output: {}",
        human::plan(report.plan()),
        existing_state(report.existing())
    )
}

enum Analyzed {
    Skip,
    Plan {
        plan: Arc<JobPlan>,
        remux_available: bool,
    },
}

async fn analyze(
    runtime: &Runtime,
    config: &EffectiveConfig,
    path: &Path,
    output: Option<PathBuf>,
    remux_only: bool,
    selector: &str,
    cancel: CancellationToken,
) -> Result<Analyzed, CliFailure> {
    let media = runtime
        .probe(path, cancel)
        .await
        .map_err(|error| runtime_failure(error, 4))?;
    let compatibility = config.compatibility_policy().map_err(config_failure)?;
    let remux_available = media.audio_streams().any(|stream| {
        matches!(
            compatibility.classify(stream),
            Ok(Compatibility::Compatible)
        )
    });
    let action = if remux_only {
        RequestedAction::RemuxOnly {
            selection: resolve_selector(&media, &compatibility, selector)?,
        }
    } else {
        RequestedAction::Convert
    };
    let policy = PlanningPolicy::new(
        compatibility,
        config.audio_target().map_err(config_failure)?,
        config.output_mode().map_err(config_failure)?,
        action,
        output.unwrap_or_else(|| default_output(path, None)),
    );
    match runtime.plan(&media, &policy) {
        Ok(PlanOutcome::Execute(plan)) => Ok(Analyzed::Plan {
            plan: Arc::new(plan),
            remux_available,
        }),
        Ok(PlanOutcome::Skip(_)) => Ok(Analyzed::Skip),
        Ok(_) => Err(CliFailure::new(5, "unknown planning outcome")),
        Err(error) => Err(runtime_failure(error, 5)),
    }
}

fn resolve_selector(
    media: &sonicmux_core::MediaInfo,
    policy: &sonicmux_core::CompatibilityPolicy,
    requested: &str,
) -> Result<AudioSelector, CliFailure> {
    if requested == "first-compatible" {
        return Ok(AudioSelector::FirstCompatible);
    }
    if let Ok(index) = requested.parse::<u32>() {
        return Ok(AudioSelector::StreamIndex(StreamIndex::new(index)));
    }
    let candidates: Result<Vec<_>, _> = media
        .audio_streams()
        .filter(|stream| {
            stream
                .common()
                .metadata()
                .language()
                .is_some_and(|language| language.as_str().eq_ignore_ascii_case(requested))
        })
        .map(|stream| {
            policy
                .classify(stream)
                .map(|classification| (stream.common().index(), classification))
        })
        .collect();
    let candidates: Vec<_> = candidates
        .map_err(|error| CliFailure::new(5, error.to_string()))?
        .into_iter()
        .filter_map(|(index, value)| matches!(value, Compatibility::Compatible).then_some(index))
        .collect();
    match candidates.as_slice() {
        [index] => Ok(AudioSelector::StreamIndex(*index)),
        [] => Err(CliFailure::new(
            5,
            format!("no compatible audio stream matches language `{requested}`"),
        )),
        many => Err(CliFailure::new(
            5,
            format!(
                "language `{requested}` is ambiguous; matching stream indices: {}",
                many.iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        )),
    }
}

async fn doctor_command(
    arguments: &DoctorArgs,
    runtime: &Runtime,
    config: &EffectiveConfig,
    output: OutputMode,
) -> Result<u8, CliFailure> {
    let encoder = config.codec.value().clone();
    let request = CapabilityRequest::new(vec![
        MediaCapability::Demuxer("matroska".to_owned()),
        MediaCapability::Muxer("matroska".to_owned()),
        MediaCapability::Decoder("dts".to_owned()),
        MediaCapability::Decoder("truehd".to_owned()),
        MediaCapability::Encoder(encoder),
    ]);
    let report = runtime
        .doctor(request, cancellation_token())
        .await
        .map_err(|error| runtime_failure(error, 3))?;
    let code = if report.all_available() { 0 } else { 3 };
    if output.json {
        emit_result(
            "doctor",
            status(code),
            json!({"report": DoctorDto::from(&report)}),
        )?;
    } else if !output.quiet {
        write_human(&human::doctor(&report, arguments.print_paths), output.color)?;
    }
    Ok(code)
}

fn config_command(
    command: &ConfigCommand,
    path: &ConfigPath,
    config: &EffectiveConfig,
    output: OutputMode,
) -> Result<u8, CliFailure> {
    match command {
        ConfigCommand::Path => {
            if output.json {
                emit_result(
                    "config",
                    "success",
                    json!({"path": PathDto::new(&path.path)}),
                )?;
            } else if !output.quiet {
                write_human(&path.path.display().to_string(), output.color)?;
            }
        }
        ConfigCommand::Init => {
            initialize_config(path).map_err(config_failure)?;
            if output.json {
                emit_result(
                    "config",
                    "success",
                    json!({"created": PathDto::new(&path.path)}),
                )?;
            } else if !output.quiet {
                write_human(&format!("Created {}", path.path.display()), output.color)?;
            }
        }
        ConfigCommand::Validate => {
            let _validated = load_file(&ConfigPath {
                path: path.path.clone(),
                required: true,
            })
            .map_err(config_failure)?;
            if output.json {
                emit_result(
                    "config",
                    "success",
                    json!({"valid": true, "path": PathDto::new(&path.path)}),
                )?;
            } else if !output.quiet {
                write_human(
                    &format!("Configuration is valid: {}", path.path.display()),
                    output.color,
                )?;
            }
        }
        ConfigCommand::Show { sources } => {
            let values = config_values(config, *sources);
            if output.json {
                emit_result("config", "success", json!({"configuration": values}))?;
            } else if !output.quiet {
                let mut text = String::new();
                for (name, value) in values.as_object().into_iter().flatten() {
                    text.push_str(&format!("{name} = {value}\n"));
                }
                write_human(text.trim_end(), output.color)?;
            }
        }
    }
    Ok(0)
}

fn config_values(config: &EffectiveConfig, sources: bool) -> Value {
    fn value<T: ToString>(value: &sonicmux_runtime::Sourced<T>, sources: bool) -> Value {
        if sources {
            json!({"value": value.value().to_string(), "source": value.source().to_string()})
        } else {
            json!(value.value().to_string())
        }
    }
    fn path_value(value: &sonicmux_runtime::Sourced<PathBuf>, sources: bool) -> Value {
        if sources {
            json!({
                "value": PathDto::new(value.value()),
                "source": value.source().to_string(),
            })
        } else {
            json!(PathDto::new(value.value()))
        }
    }
    json!({
        "profile": value(&config.profile, sources),
        "codec": value(&config.codec, sources),
        "bitrate": value(&config.bitrate, sources),
        "channels": value(&config.channels, sources),
        "mode": value(&config.mode, sources),
        "color": value(&config.color, sources),
        "ffmpeg_path": config.ffmpeg_path.as_ref().map(|item| path_value(item, sources)),
        "output_directory": config.output_directory.as_ref().map(|item| path_value(item, sources)),
        "log_file": config.log_file.as_ref().map(|item| path_value(item, sources)),
        "jobs": value(&config.jobs, sources),
        "storage_profile": value(&config.storage_profile, sources),
    })
}

fn presets_command(
    command: &PresetsCommand,
    config: &EffectiveConfig,
    output: OutputMode,
) -> Result<u8, CliFailure> {
    match command {
        PresetsCommand::List => {
            let names: Vec<_> = config.profile_names().collect();
            if output.json {
                emit_result("presets", "success", json!({"presets": names}))?;
            } else if !output.quiet {
                write_human(&names.join("\n"), output.color)?;
            }
        }
        PresetsCommand::Show { name } => {
            let policy = config
                .compatibility_policy_named(name)
                .map_err(config_failure)?;
            let unknown_codec = match policy.unknown_codec_behavior() {
                sonicmux_core::UnknownCodecBehavior::Reject => "reject",
                sonicmux_core::UnknownCodecBehavior::TranscodeWithFallback => {
                    "transcode-with-fallback"
                }
                _ => "unknown",
            };
            let mut rules = Vec::new();
            let mut rule_lines = Vec::new();
            for (family, rule) in policy.rules() {
                let maximum = rule.maximum_channels().map(|value| value.get());
                let layouts = rule
                    .allowed_layouts()
                    .map(|values| values.iter().cloned().collect::<Vec<_>>());
                rule_lines.push(format!(
                    "  {}: maximum channels {}; layouts {}",
                    family.label(),
                    maximum.map_or_else(|| "any".to_owned(), |value| value.to_string()),
                    layouts
                        .as_ref()
                        .map_or_else(|| "any".to_owned(), |values| values.join(","))
                ));
                rules.push(json!({
                    "codec": family.label(),
                    "maximum_channels": maximum,
                    "allowed_layouts": layouts,
                }));
            }
            if output.json {
                emit_result(
                    "presets",
                    "success",
                    json!({
                        "name": name,
                        "description": policy.description(),
                        "conservative": policy.is_conservative_baseline(),
                        "unknown_codec": unknown_codec,
                        "rules": rules,
                    }),
                )?;
            } else if !output.quiet {
                let mut lines = vec![
                    format!("{name}: {}", policy.description()),
                    format!("  unknown codec: {unknown_codec}"),
                ];
                if policy.is_conservative_baseline() {
                    lines.push("  warning: support varies by model and firmware".to_owned());
                }
                lines.extend(rule_lines);
                write_human(&lines.join("\n"), output.color)?;
            }
        }
    }
    Ok(0)
}

fn generate_artifact(command: &Command) -> Result<u8, CliFailure> {
    match command {
        Command::Completions(arguments) => {
            let shell = match arguments.shell {
                CompletionShell::Bash => clap_complete::Shell::Bash,
                CompletionShell::Elvish => clap_complete::Shell::Elvish,
                CompletionShell::Fish => clap_complete::Shell::Fish,
                CompletionShell::Powershell => clap_complete::Shell::PowerShell,
                CompletionShell::Zsh => clap_complete::Shell::Zsh,
            };
            clap_complete::generate(shell, &mut Cli::command(), "sonicmux", &mut io::stdout());
        }
        Command::Man(arguments) => {
            let man = clap_mangen::Man::new(Cli::command());
            match &arguments.output {
                Some(path) => {
                    let mut file = OpenOptions::new()
                        .create_new(true)
                        .write(true)
                        .open(path)
                        .map_err(|error| {
                            CliFailure::new(
                                2,
                                format!("failed to create {}: {error}", path.display()),
                            )
                        })?;
                    man.render(&mut file)
                        .map_err(|error| CliFailure::new(6, error.to_string()))?;
                    file.sync_all()
                        .map_err(|error| CliFailure::new(6, error.to_string()))?;
                }
                None => man
                    .render(&mut io::stdout())
                    .map_err(|error| CliFailure::new(6, error.to_string()))?,
            }
        }
        _ => return Err(CliFailure::new(2, "not an artifact command")),
    }
    Ok(0)
}

fn discovery_request(inputs: &[OsString], options: &args::DiscoveryArgs) -> DiscoveryRequest {
    DiscoveryRequest {
        roots: inputs.to_vec(),
        recursive: options.recursive,
        follow_links: options.follow_links,
        includes: options.include.clone(),
        excludes: options.exclude.clone(),
    }
}

fn resolve_output(
    input: &Path,
    arguments: &ConvertArgs,
    config: &EffectiveConfig,
) -> Result<PathBuf, CliFailure> {
    if let Some(output) = &arguments.output {
        return Ok(output.clone());
    }
    let directory = arguments.output_dir.as_deref().or_else(|| {
        config
            .output_directory
            .as_ref()
            .map(|value| value.value().as_path())
    });
    Ok(default_output(input, directory))
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

fn validate_probe_input(path: &Path) -> Result<(), CliFailure> {
    let metadata = std::fs::symlink_metadata(path).map_err(|error| {
        CliFailure::new(4, format!("failed to inspect {}: {error}", path.display()))
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(CliFailure::new(4, "probe input must be a regular file"));
    }
    if !path
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case("mkv"))
    {
        return Err(CliFailure::new(4, "probe accepts only .mkv files"));
    }
    Ok(())
}

fn cancellation_token() -> CancellationToken {
    let token = CancellationToken::new();
    let signal_token = token.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            signal_token.cancel();
        }
    });
    token
}

fn runtime_failure(error: RuntimeError, fallback: u8) -> CliFailure {
    let code = if is_cancelled(&error) {
        130
    } else {
        match &error {
            RuntimeError::Backend(sonicmux_backend::BackendError::Probe { .. }) => 4,
            RuntimeError::Backend(sonicmux_backend::BackendError::Capability { .. }) => 3,
            RuntimeError::Backend(sonicmux_backend::BackendError::Execute { .. }) => 6,
            RuntimeError::Plan(_) => 5,
            RuntimeError::Execution(_) | RuntimeError::InspectOutput { .. } => 6,
            _ => fallback,
        }
    };
    CliFailure::new(code, error.to_string())
}

fn is_cancelled(error: &RuntimeError) -> bool {
    matches!(
        error,
        RuntimeError::Backend(sonicmux_backend::BackendError::Cancelled)
            | RuntimeError::Execution(sonicmux_runtime::ExecutionError::Cancelled)
    )
}

fn config_failure(error: ConfigError) -> CliFailure {
    CliFailure::new(2, error.to_string())
}

fn batch_code(total: usize, failures: usize, single_code: u8) -> u8 {
    match failures {
        0 => 0,
        _ if total > 1 => 1,
        _ => single_code,
    }
}

fn status(code: u8) -> &'static str {
    match code {
        0 => "success",
        1 => "partial",
        130 => "cancelled",
        _ => "failure",
    }
}

fn command_name(command: &Command) -> &'static str {
    match command {
        Command::Probe(_) => "probe",
        Command::Convert(_) => "convert",
        Command::Scan(_) => "scan",
        Command::Config(_) => "config",
        Command::Presets(_) => "presets",
        Command::Doctor(_) => "doctor",
        Command::Completions(_) => "completions",
        Command::Man(_) => "man",
    }
}

fn existing_state(value: &ExistingOutputOutcome) -> &'static str {
    match value {
        ExistingOutputOutcome::Absent => "absent",
        ExistingOutputOutcome::Valid => "valid",
        ExistingOutputOutcome::Conflict { .. } => "conflict",
    }
}

fn error_value(path: &Path, error: &CliFailure) -> Value {
    json!({
        "path": PathDto::new(path),
        "status": "failure",
        "exit_code": error.code,
        "message": error.message,
    })
}

fn emit_file_success(
    output: OutputMode,
    sequence: &Arc<AtomicU64>,
    path: &Path,
    state: &str,
) -> Result<(), CliFailure> {
    if output.json_progress {
        emit_event(
            sequence,
            "file_succeeded",
            json!({"path": PathDto::new(path), "status": state}),
        )
    } else if !output.json && !output.quiet {
        write_human(&format!("{}: {state}", path.display()), output.color)
    } else {
        Ok(())
    }
}

fn emit_file_failure(
    output: OutputMode,
    sequence: &Arc<AtomicU64>,
    path: &Path,
    error: &CliFailure,
) -> Result<(), CliFailure> {
    if output.json_progress {
        emit_event(sequence, "file_failed", error_value(path, error))
    } else if !output.json {
        eprintln!("error: {}: {}", path.display(), error.message);
        Ok(())
    } else {
        Ok(())
    }
}

fn emit_result<T: Serialize>(command: &str, status: &str, data: T) -> Result<(), CliFailure> {
    write_json(&json!({
        "schema": "sonicmux.result",
        "version": 1,
        "command": command,
        "status": status,
        "data": data,
    }))
}

fn emit_event<T: Serialize>(
    sequence: &Arc<AtomicU64>,
    event: &str,
    data: T,
) -> Result<(), CliFailure> {
    let sequence = sequence.fetch_add(1, Ordering::Relaxed);
    write_json(&json!({
        "schema": "sonicmux.event",
        "version": 1,
        "sequence": sequence,
        "event": event,
        "data": data,
    }))
}

fn write_json<T: Serialize>(value: &T) -> Result<(), CliFailure> {
    let mut bytes = serde_json::to_vec(value)
        .map_err(|error| CliFailure::new(6, format!("failed to serialize output: {error}")))?;
    bytes.push(b'\n');
    io::stdout()
        .lock()
        .write_all(&bytes)
        .map_err(|error| CliFailure::new(6, format!("failed to write stdout: {error}")))
}

fn write_human(value: &str, color: bool) -> Result<(), CliFailure> {
    let choice = if color {
        anstream::ColorChoice::Always
    } else {
        anstream::ColorChoice::Never
    };
    let mut stdout = anstream::AutoStream::new(io::stdout(), choice).lock();
    let rendered = if color {
        format!("\u{1b}[32m{value}\u{1b}[0m\n")
    } else {
        format!("{value}\n")
    };
    stdout
        .write_all(rendered.as_bytes())
        .map_err(|error| CliFailure::new(6, format!("failed to write stdout: {error}")))
}

fn color_enabled(choice: &str) -> bool {
    match choice {
        "always" => true,
        "never" => false,
        _ => io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none(),
    }
}

fn emit_failure(output: OutputMode, command: &str, error: &CliFailure) {
    if output.json {
        let _ignored = emit_result(
            command,
            status(error.code),
            json!({"exit_code": error.code, "message": error.message}),
        );
    } else if output.json_progress {
        let sequence = Arc::new(AtomicU64::new(0));
        let _ignored = emit_event(
            &sequence,
            "batch_failed",
            json!({"exit_code": error.code, "message": error.message}),
        );
    } else {
        eprintln!("error: {}", error.message);
    }
}

#[allow(dead_code)]
fn _defaults_are_valid() -> Result<EffectiveConfig, ConfigError> {
    merge_config(
        DefaultConfig::default(),
        PartialConfig::default(),
        PartialConfig::default(),
        PartialConfig::default(),
    )
}
