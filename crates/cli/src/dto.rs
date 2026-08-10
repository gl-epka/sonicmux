//! Explicit versioned machine-output DTOs.

use std::path::Path;

use serde::Serialize;
use sonicmux_backend::{BackendCapabilities, BackendToolRole, ProgressSnapshot};
use sonicmux_core::{MediaInfo, OutputStreamPlan, StreamInfo};

/// Exact and display-safe path representation.
#[derive(Debug, Serialize)]
pub struct PathDto {
    display: String,
    native_encoding: &'static str,
    native_hex: String,
}

impl PathDto {
    /// Converts one native path without fallible Unicode assumptions.
    #[must_use]
    pub fn new(path: &Path) -> Self {
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt as _;
            let bytes = path.as_os_str().as_bytes();
            let native_encoding = if std::str::from_utf8(bytes).is_ok() {
                "utf-8"
            } else {
                "unix-bytes"
            };
            Self {
                display: path.to_string_lossy().into_owned(),
                native_encoding,
                native_hex: encode_hex(bytes.iter().copied()),
            }
        }
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStrExt as _;
            let units: Vec<u16> = path.as_os_str().encode_wide().collect();
            let exact_utf8 = path.to_str().is_some();
            let bytes = if exact_utf8 {
                path.to_str().unwrap_or_default().as_bytes().to_vec()
            } else {
                units.iter().flat_map(|unit| unit.to_le_bytes()).collect()
            };
            Self {
                display: path.to_string_lossy().into_owned(),
                native_encoding: if exact_utf8 {
                    "utf-8"
                } else {
                    "windows-wtf16le"
                },
                native_hex: encode_hex(bytes),
            }
        }
    }
}

/// Stable probe document payload.
#[derive(Debug, Serialize)]
pub struct ProbeDto {
    path: PathDto,
    formats: Vec<String>,
    duration_us: Option<u64>,
    bitrate_bps: Option<u64>,
    chapters: usize,
    warnings: usize,
    streams: Vec<StreamDto>,
}

#[derive(Debug, Serialize)]
struct StreamDto {
    index: u32,
    kind: String,
    codec: String,
    channels: Option<u16>,
    layout: Option<String>,
    bitrate_bps: Option<u64>,
    language: Option<String>,
    title: Option<String>,
    default: bool,
}

impl From<&MediaInfo> for ProbeDto {
    fn from(media: &MediaInfo) -> Self {
        Self {
            path: PathDto::new(media.path()),
            formats: media.format().names().to_vec(),
            duration_us: media.format().duration().map(|value| value.get()),
            bitrate_bps: media.format().bitrate().map(|value| value.get()),
            chapters: media.chapters().len(),
            warnings: media.warnings().len(),
            streams: media.streams().iter().map(StreamDto::from).collect(),
        }
    }
}

impl From<&StreamInfo> for StreamDto {
    fn from(stream: &StreamInfo) -> Self {
        let common = stream.common();
        let (kind, codec, channels, layout) = match stream {
            StreamInfo::Video(_) => ("video", common.codec_name().to_owned(), None, None),
            StreamInfo::Audio(audio) => (
                "audio",
                audio.codec().to_string(),
                Some(audio.channels().count().get()),
                audio.channels().layout_name().map(str::to_owned),
            ),
            StreamInfo::Subtitle(_) => ("subtitle", common.codec_name().to_owned(), None, None),
            StreamInfo::Attachment(_) => ("attachment", common.codec_name().to_owned(), None, None),
            StreamInfo::Data(_) => ("data", common.codec_name().to_owned(), None, None),
            StreamInfo::Unknown(value) => {
                (value.kind(), common.codec_name().to_owned(), None, None)
            }
            _ => ("unknown", common.codec_name().to_owned(), None, None),
        };
        Self {
            index: stream.index().get(),
            kind: kind.to_owned(),
            codec,
            channels,
            layout,
            bitrate_bps: common.bitrate().map(|value| value.get()),
            language: common.metadata().language().map(|value| value.to_string()),
            title: common.metadata().title().map(str::to_owned),
            default: common.dispositions().is_default(),
        }
    }
}

/// Stable plan payload used by dry-run and scan.
#[derive(Debug, Serialize)]
pub struct PlanDto {
    input: PathDto,
    output: PathDto,
    action: &'static str,
    duration_us: Option<u64>,
    streams: usize,
    encoded_audio_streams: usize,
}

impl From<&sonicmux_core::JobPlan> for PlanDto {
    fn from(plan: &sonicmux_core::JobPlan) -> Self {
        Self {
            input: PathDto::new(plan.input()),
            output: PathDto::new(plan.output()),
            action: match plan.action() {
                sonicmux_core::JobAction::Transcode => "transcode",
                sonicmux_core::JobAction::RemuxOnly => "remux",
                _ => "unknown",
            },
            duration_us: plan.duration().map(|value| value.get()),
            streams: plan.streams().len(),
            encoded_audio_streams: plan
                .streams()
                .iter()
                .filter(|stream| matches!(stream, OutputStreamPlan::EncodeAudio { .. }))
                .count(),
        }
    }
}

/// Stable backend diagnostic payload.
#[derive(Debug, Serialize)]
pub struct DoctorDto {
    backend: String,
    healthy: bool,
    tools: Vec<ToolDto>,
    checks: Vec<CheckDto>,
    warnings: Vec<String>,
}

#[derive(Debug, Serialize)]
struct ToolDto {
    role: &'static str,
    path: PathDto,
    version: Option<String>,
}

#[derive(Debug, Serialize)]
struct CheckDto {
    kind: String,
    name: String,
    available: bool,
    detail: Option<String>,
}

impl From<&BackendCapabilities> for DoctorDto {
    fn from(report: &BackendCapabilities) -> Self {
        Self {
            backend: report.backend_name().to_owned(),
            healthy: report.all_available(),
            tools: report
                .tools()
                .iter()
                .map(|tool| ToolDto {
                    role: match tool.role() {
                        BackendToolRole::Ffmpeg => "ffmpeg",
                        BackendToolRole::Ffprobe => "ffprobe",
                    },
                    path: PathDto::new(tool.path()),
                    version: tool.version().map(str::to_owned),
                })
                .collect(),
            checks: report
                .checks()
                .iter()
                .map(|check| CheckDto {
                    kind: check.capability().kind().to_owned(),
                    name: check.capability().name().to_owned(),
                    available: check.available(),
                    detail: check.detail().map(str::to_owned),
                })
                .collect(),
            warnings: report.warnings().to_vec(),
        }
    }
}

/// Progress snapshot with explicit units.
#[derive(Debug, Serialize)]
pub struct ProgressDto {
    out_time_us: Option<i64>,
    total_size_bytes: Option<u64>,
    speed_milli: Option<u32>,
    frame: Option<u64>,
    dropped_frames: Option<u64>,
}

impl From<&ProgressSnapshot> for ProgressDto {
    fn from(value: &ProgressSnapshot) -> Self {
        Self {
            out_time_us: value.out_time_us,
            total_size_bytes: value.total_size_bytes,
            speed_milli: value.speed_milli,
            frame: value.frame,
            dropped_frames: value.dropped_frames,
        }
    }
}

fn encode_hex(bytes: impl IntoIterator<Item = u8>) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    bytes
        .into_iter()
        .flat_map(|byte| {
            [
                HEX[(byte >> 4) as usize] as char,
                HEX[(byte & 0x0f) as usize] as char,
            ]
        })
        .collect()
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests {
    use super::PathDto;

    #[test]
    fn utf8_path_has_exact_bytes() {
        let value = serde_json::to_value(PathDto::new(std::path::Path::new("movie.mkv")))
            .expect("path DTO serializes");
        assert_eq!(value["native_encoding"], "utf-8");
        assert_eq!(value["native_hex"], "6d6f7669652e6d6b76");
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_unix_path_is_lossless() {
        use std::os::unix::ffi::OsStrExt as _;

        let path = std::path::Path::new(std::ffi::OsStr::from_bytes(b"bad-\xff.mkv"));
        let value = serde_json::to_value(PathDto::new(path)).expect("path DTO serializes");
        assert_eq!(value["native_encoding"], "unix-bytes");
        assert_eq!(value["native_hex"], "6261642dff2e6d6b76");
    }
}
