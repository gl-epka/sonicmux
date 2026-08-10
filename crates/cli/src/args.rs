//! Command-line syntax and clap validation.

use std::{ffi::OsString, path::PathBuf};

use clap::{Args, Parser, Subcommand, ValueEnum};

/// Make MKV audio playable on TVs without re-encoding video.
#[derive(Debug, Parser)]
#[command(
    name = "sonicmux",
    bin_name = "sonicmux",
    version,
    about = "Make MKV audio playable on TVs without re-encoding video.",
    arg_required_else_help = true
)]
pub struct Cli {
    /// Use this TOML configuration file.
    #[arg(long, global = true, value_name = "PATH")]
    pub config: Option<PathBuf>,
    /// Use this FFmpeg executable or installation directory.
    #[arg(long, global = true, value_name = "PATH")]
    pub ffmpeg_path: Option<PathBuf>,
    /// Write the final result as JSON to stdout.
    #[arg(long, global = true)]
    pub json: bool,
    /// Write versioned NDJSON events to stdout.
    #[arg(long, global = true, conflicts_with = "json")]
    pub json_progress: bool,
    /// Color output.
    #[arg(long, global = true, value_enum)]
    pub color: Option<ColorChoice>,
    /// Also write structured diagnostic logs to a file.
    #[arg(long, global = true, value_name = "PATH")]
    pub log_file: Option<PathBuf>,
    /// Increase diagnostic verbosity.
    #[arg(short, long, global = true, action = clap::ArgAction::Count)]
    pub verbose: u8,
    /// Hide non-error human output.
    #[arg(short, long, global = true)]
    pub quiet: bool,
    /// Operation to perform.
    #[command(subcommand)]
    pub command: Command,
}

/// Top-level SonicMux operation.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Inspect streams, chapters, attachments, and metadata.
    Probe(ProbeArgs),
    /// Convert incompatible audio or remux existing compatible audio.
    Convert(ConvertArgs),
    /// Find MKV files and show the actions they require.
    Scan(ScanArgs),
    /// Inspect and manage configuration.
    Config(ConfigArgs),
    /// List and inspect device presets.
    Presets(PresetsArgs),
    /// Check FFmpeg and required codec capabilities.
    Doctor(DoctorArgs),
    /// Generate shell completion scripts.
    Completions(CompletionsArgs),
    /// Generate a manual page.
    Man(ManArgs),
}

/// Arguments for media inspection.
#[derive(Debug, Args)]
pub struct ProbeArgs {
    /// MKV files to inspect.
    #[arg(required = true, value_name = "INPUT")]
    pub inputs: Vec<OsString>,
    /// Show one summary row per file.
    #[arg(long)]
    pub compact: bool,
}

/// Shared compatibility and target overrides.
#[derive(Debug, Args, Clone, Default)]
pub struct AudioArgs {
    /// Device profile.
    #[arg(long, value_name = "PROFILE")]
    pub profile: Option<String>,
    /// Target audio codec.
    #[arg(long, value_parser = ["ac3", "eac3", "aac"])]
    pub codec: Option<String>,
    /// Target bitrate, for example 640k.
    #[arg(long, value_name = "RATE")]
    pub bitrate: Option<String>,
    /// Output layout.
    #[arg(long, value_parser = ["keep-up-to-5.1", "stereo", "5.1"])]
    pub channels: Option<String>,
    /// Audio output mode.
    #[arg(long, value_parser = ["add", "replace", "only-new"])]
    pub mode: Option<String>,
}

/// Shared input discovery controls.
#[derive(Debug, Args, Clone, Default)]
pub struct DiscoveryArgs {
    /// Recurse into input directories.
    #[arg(short, long)]
    pub recursive: bool,
    /// Include matching relative paths.
    #[arg(long, value_name = "GLOB")]
    pub include: Vec<String>,
    /// Exclude matching relative paths.
    #[arg(long, value_name = "GLOB")]
    pub exclude: Vec<String>,
    /// Follow symbolic links.
    #[arg(long)]
    pub follow_links: bool,
}

/// Arguments for conversion/remux.
#[derive(Debug, Args)]
pub struct ConvertArgs {
    /// MKV files, directories, or glob patterns.
    #[arg(required = true, value_name = "INPUT")]
    pub inputs: Vec<OsString>,
    /// Compatibility and target settings.
    #[command(flatten)]
    pub audio: AudioArgs,
    /// Input discovery settings.
    #[command(flatten)]
    pub discovery: DiscoveryArgs,
    /// Remux an existing compatible audio track without encoding.
    #[arg(long)]
    pub remux_only: bool,
    /// Track for remux: index, language, or first-compatible.
    #[arg(long, default_value = "first-compatible", requires = "remux_only")]
    pub default_audio: String,
    /// Exact output path; requires one discovered input.
    #[arg(short, long, value_name = "PATH", conflicts_with = "output_dir")]
    pub output: Option<PathBuf>,
    /// Put generated outputs in this existing directory.
    #[arg(long, value_name = "DIR", conflicts_with = "output")]
    pub output_dir: Option<PathBuf>,
    /// Probe and print plans without writing files.
    #[arg(long)]
    pub dry_run: bool,
    /// Maximum files processed at once.
    #[arg(long, value_name = "N", value_parser = parse_jobs)]
    pub jobs: Option<usize>,
    /// Storage concurrency profile.
    #[arg(long, value_enum)]
    pub storage_profile: Option<StorageProfileChoice>,
    /// Continue the batch after a file fails (default).
    #[arg(long, conflicts_with = "fail_fast")]
    pub continue_on_error: bool,
    /// Stop admission and cancel active files after the first failure.
    #[arg(long, conflicts_with = "continue_on_error")]
    pub fail_fast: bool,
}

/// Arguments for read-only scanning.
#[derive(Debug, Args)]
pub struct ScanArgs {
    /// MKV files, directories, or glob patterns.
    #[arg(required = true, value_name = "PATH")]
    pub inputs: Vec<OsString>,
    /// Compatibility and target settings.
    #[command(flatten)]
    pub audio: AudioArgs,
    /// Input discovery settings.
    #[command(flatten)]
    pub discovery: DiscoveryArgs,
    /// Show only files that need work.
    #[arg(long)]
    pub needs_action: bool,
}

/// Configuration command group.
#[derive(Debug, Args)]
pub struct ConfigArgs {
    /// Configuration action.
    #[command(subcommand)]
    pub command: ConfigCommand,
}

/// Configuration action.
#[derive(Debug, Subcommand)]
pub enum ConfigCommand {
    /// Print the effective merged configuration.
    Show {
        /// Annotate every value with its winning source.
        #[arg(long)]
        sources: bool,
    },
    /// Print the active configuration path.
    Path,
    /// Write a documented starter configuration without replacing a file.
    Init,
    /// Validate the selected configuration file.
    Validate,
}

/// Preset command group.
#[derive(Debug, Args)]
pub struct PresetsArgs {
    /// Preset action.
    #[command(subcommand)]
    pub command: PresetsCommand,
}

/// Preset action.
#[derive(Debug, Subcommand)]
pub enum PresetsCommand {
    /// List built-in and configured presets.
    List,
    /// Show compatibility rules for one preset.
    Show {
        /// Built-in or configured preset name.
        name: String,
    },
}

/// Backend diagnostics.
#[derive(Debug, Args)]
pub struct DoctorArgs {
    /// Device profile to check.
    #[arg(long)]
    pub profile: Option<String>,
    /// Target encoder to check.
    #[arg(long, value_parser = ["ac3", "eac3", "aac"])]
    pub codec: Option<String>,
    /// Always include executable paths in human output.
    #[arg(long)]
    pub print_paths: bool,
}

/// Completion generation.
#[derive(Debug, Args)]
pub struct CompletionsArgs {
    /// Target shell.
    #[arg(value_enum)]
    pub shell: CompletionShell,
}

/// Supported completion shell.
#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum CompletionShell {
    /// Bash.
    Bash,
    /// Elvish.
    Elvish,
    /// Fish.
    Fish,
    /// PowerShell.
    Powershell,
    /// Zsh.
    Zsh,
}

/// Manual generation.
#[derive(Debug, Args)]
pub struct ManArgs {
    /// Write ROFF to a new file instead of stdout.
    #[arg(short, long, value_name = "PATH")]
    pub output: Option<PathBuf>,
}

/// Color policy.
#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum ColorChoice {
    /// Detect terminal support and `NO_COLOR`.
    Auto,
    /// Always emit terminal color.
    Always,
    /// Never emit terminal color.
    Never,
}

impl ColorChoice {
    /// Returns the stable configuration spelling.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Always => "always",
            Self::Never => "never",
        }
    }
}

/// Coarse storage intent for the scheduler default.
#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum StorageProfileChoice {
    /// Rotational or seek-sensitive storage; defaults to one job.
    Hdd,
    /// Conservative general-purpose concurrency.
    Balanced,
    /// Explicit solid-state storage intent.
    Nvme,
}

impl StorageProfileChoice {
    /// Returns the stable configuration spelling.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Hdd => "hdd",
            Self::Balanced => "balanced",
            Self::Nvme => "nvme",
        }
    }
}

fn parse_jobs(value: &str) -> Result<usize, String> {
    value
        .parse::<usize>()
        .ok()
        .filter(|jobs| (1..=64).contains(jobs))
        .ok_or_else(|| "expected an integer from 1 through 64".to_owned())
}
