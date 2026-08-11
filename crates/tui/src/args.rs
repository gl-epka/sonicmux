//! Command-line bootstrap options for the interactive frontend.

use std::{ffi::OsString, path::PathBuf};

use clap::Parser;

/// Launch the keyboard-first SonicMux terminal interface.
#[derive(Debug, Parser)]
#[command(
    name = "sonicmux-tui",
    version,
    about = "Interactive MKV audio conversion without video re-encoding"
)]
pub struct TuiArgs {
    /// MKV files, directories, or glob patterns to add at startup.
    #[arg(value_name = "INPUT")]
    pub inputs: Vec<OsString>,
    /// Use this TOML configuration file.
    #[arg(long, value_name = "PATH")]
    pub config: Option<PathBuf>,
    /// Use this FFmpeg executable or installation directory.
    #[arg(long, value_name = "PATH")]
    pub ffmpeg_path: Option<PathBuf>,
    /// Also write structured diagnostic logs to this file.
    #[arg(long, value_name = "PATH")]
    pub log_file: Option<PathBuf>,
    /// Put generated outputs in this existing directory.
    #[arg(long, value_name = "DIR")]
    pub output_dir: Option<PathBuf>,
    /// Descend below direct directory children.
    #[arg(short, long)]
    pub recursive: bool,
    /// Follow explicit and encountered symbolic links.
    #[arg(long)]
    pub follow_links: bool,
    /// Include matching relative paths.
    #[arg(long, value_name = "GLOB")]
    pub include: Vec<String>,
    /// Exclude matching relative paths.
    #[arg(long, value_name = "GLOB")]
    pub exclude: Vec<String>,
    /// Probe and show plans without writing files.
    #[arg(long)]
    pub dry_run: bool,
    /// Disable ANSI colors while retaining textual status labels.
    #[arg(long)]
    pub no_color: bool,
}
