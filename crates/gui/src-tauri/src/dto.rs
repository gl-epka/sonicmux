//! Stable serialization contracts between Rust and the local webview.

use serde::{Deserialize, Serialize};

/// Current desktop session phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum AppPhaseDto {
    /// FFmpeg is unavailable and setup is required.
    ToolchainSetup,
    /// No batch is active.
    Idle,
    /// Files are being discovered or probed.
    Probing,
    /// The bounded scheduler is active.
    Running,
    /// Cancellation was requested and cleanup is pending.
    Cancelling,
}

/// One frontend-safe FFmpeg discovery result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolchainStatusDto {
    /// Whether both executables are available.
    pub available: bool,
    /// Stable source spelling or `missing`.
    pub source: String,
    /// Human-readable bounded detail.
    pub detail: String,
}

/// Editable, session-scoped conversion controls.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsDto {
    /// Device compatibility profile.
    pub profile: String,
    /// `convert` or `remux`.
    pub action: String,
    /// Target audio codec.
    pub codec: String,
    /// Target bitrate spelling.
    pub bitrate: String,
    /// Target channel layout spelling.
    pub channels: String,
    /// Output audio mode.
    pub mode: String,
    /// Maximum concurrent files.
    pub jobs: usize,
    /// Storage concurrency profile.
    pub storage_profile: String,
    /// Failure behavior.
    pub failure_policy: String,
    /// Whether execution is disabled.
    pub dry_run: bool,
    /// Lossy display of the selected output directory.
    pub output_directory: Option<String>,
}

/// A media stream rendered in the selected-file inspector.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackDto {
    /// Container stream index.
    pub index: u32,
    /// Stable stream kind.
    pub kind: String,
    /// Codec display name.
    pub codec: String,
    /// Audio channel count when applicable.
    pub channels: Option<u16>,
    /// Source language tag when present.
    pub language: Option<String>,
    /// Source title when present.
    pub title: Option<String>,
    /// Whether the source marks this stream as default.
    pub default: bool,
    /// Planned output action.
    pub action: String,
}

/// One frontend-safe queue row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueItemDto {
    /// Session-local stable identifier.
    pub id: u64,
    /// Lossy filename display.
    pub name: String,
    /// Lossy full input display for tooltips.
    pub input_display: String,
    /// Lossy output display.
    pub output_display: String,
    /// Whether the item participates in the next batch.
    pub enabled: bool,
    /// Stable lifecycle spelling.
    pub status: String,
    /// Progress where 1,000 is complete.
    pub progress_milli: Option<u16>,
    /// Estimated remaining seconds.
    pub eta_seconds: Option<u64>,
    /// Short plan summary.
    pub plan: String,
    /// Bounded recoverable error.
    pub error: Option<String>,
    /// Probed stream details.
    pub tracks: Vec<TrackDto>,
}

/// Authoritative render state for the main window.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSnapshotDto {
    /// Current state-machine phase.
    pub phase: AppPhaseDto,
    /// Current queue rows in discovery order.
    pub queue: Vec<QueueItemDto>,
    /// Effective session settings.
    pub settings: SettingsDto,
    /// Available profile names.
    pub profiles: Vec<String>,
    /// Whether Start is currently valid.
    pub can_start: bool,
    /// Aggregate progress where 1,000 is complete.
    pub progress_milli: Option<u16>,
    /// Aggregate ETA in seconds.
    pub eta_seconds: Option<u64>,
    /// Recent bounded lifecycle messages.
    pub logs: Vec<String>,
}

/// Initial GUI response and toolchain health.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BootstrapDto {
    /// Versioned DTO schema identifier.
    pub schema: &'static str,
    /// Application version.
    pub version: &'static str,
    /// Toolchain discovery result.
    pub toolchain: ToolchainStatusDto,
    /// Authoritative initial state.
    pub snapshot: SessionSnapshotDto,
}

/// Ordered messages sent through the webview channel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", tag = "event", content = "data")]
pub enum GuiEventDto {
    /// State replacement after a Rust-side transition.
    Snapshot(Box<SessionSnapshotDto>),
    /// An assertive or polite human-readable notice.
    Notice {
        /// `info`, `warning`, or `error`.
        level: String,
        /// Bounded message with a recovery action.
        message: String,
    },
    /// A native application menu item requested a frontend-owned action.
    Menu {
        /// Stable action identifier.
        action: String,
    },
}

/// Kind of native input picker requested by the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PickKindDto {
    /// Select one or more Matroska files.
    Files,
    /// Select one directory.
    Directory,
}

/// A command was accepted for asynchronous completion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AcceptedDto {
    /// Always true for a successful command return.
    pub accepted: bool,
}

impl AcceptedDto {
    /// Creates a successful acknowledgement.
    #[must_use]
    pub const fn yes() -> Self {
        Self { accepted: true }
    }
}

#[cfg(test)]
mod tests {
    use super::{AppPhaseDto, GuiEventDto, SessionSnapshotDto, SettingsDto};

    fn snapshot() -> SessionSnapshotDto {
        SessionSnapshotDto {
            phase: AppPhaseDto::Idle,
            queue: Vec::new(),
            settings: SettingsDto {
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
            },
            profiles: vec!["generic-tv".to_owned()],
            can_start: false,
            progress_milli: None,
            eta_seconds: None,
            logs: Vec::new(),
        }
    }

    #[test]
    fn event_contract_uses_tagged_camel_case_json() -> Result<(), serde_json::Error> {
        let value = serde_json::to_value(GuiEventDto::Snapshot(Box::new(snapshot())))?;

        assert_eq!(value["event"], "snapshot");
        assert_eq!(value["data"]["phase"], "idle");
        assert_eq!(value["data"]["settings"]["storageProfile"], "balanced");
        assert!(value["data"].get("storage_profile").is_none());
        Ok(())
    }
}
