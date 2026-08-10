//! FFprobe process protocol and JSON-to-domain conversion.

use std::{
    collections::BTreeMap,
    env,
    ffi::OsString,
    io,
    path::{Path, PathBuf},
    process::Stdio,
};

use command_group::{AsyncCommandGroup as _, AsyncGroupChild};
use serde::Deserialize;
use serde_json::Value;
use sonicmux_core::{
    AttachmentStream, AudioCodec, AudioStream, Bitrate, ChannelCount, Channels, Chapter,
    DataStream, Dispositions, DurationMicros, FormatInfo, MediaInfo, MediaTimestamp, Metadata,
    ModelError, ProbeWarning, SampleRate, StreamCommon, StreamIndex, StreamInfo, StreamTiming,
    SubtitleStream, TimeBase, UnknownStream, VideoStream,
};
use thiserror::Error;
use tokio::{
    io::{AsyncRead, AsyncReadExt as _},
    process::Command,
    task::JoinError,
    time::{Duration, sleep},
};
use tokio_util::sync::CancellationToken;

const MAX_PROBE_STDOUT_BYTES: usize = 64 * 1024 * 1024;
const MAX_PROBE_STDERR_BYTES: usize = 64 * 1024;
const PROCESS_POLL_INTERVAL: Duration = Duration::from_millis(25);

/// Error produced while invoking FFprobe or interpreting its result.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ProbeError {
    /// The current working directory could not be obtained for a relative path.
    #[error("failed to resolve input path: {source}")]
    ResolveInput {
        /// Operating-system error.
        #[source]
        source: io::Error,
    },
    /// FFprobe could not be launched.
    #[error("failed to launch FFprobe at {executable}: {source}")]
    Spawn {
        /// Configured executable path.
        executable: PathBuf,
        /// Operating-system error.
        #[source]
        source: io::Error,
    },
    /// A configured child pipe was unexpectedly unavailable.
    #[error("FFprobe {pipe} pipe was unavailable")]
    MissingPipe {
        /// Pipe name used in diagnostics.
        pipe: &'static str,
    },
    /// Reading FFprobe output failed.
    #[error("failed to read FFprobe {pipe}: {source}")]
    ReadOutput {
        /// Pipe name used in diagnostics.
        pipe: &'static str,
        /// Operating-system error.
        #[source]
        source: io::Error,
    },
    /// FFprobe JSON exceeded the memory safety limit.
    #[error("FFprobe JSON exceeds the {limit_bytes}-byte safety limit")]
    OutputTooLarge {
        /// Configured limit.
        limit_bytes: usize,
    },
    /// Waiting for FFprobe failed.
    #[error("failed to wait for FFprobe: {source}")]
    Wait {
        /// Operating-system error.
        #[source]
        source: io::Error,
    },
    /// The bounded stderr reader task failed unexpectedly.
    #[error("FFprobe stderr reader failed: {message}")]
    StderrTask {
        /// Join or read diagnostic.
        message: String,
    },
    /// The bounded stdout reader task failed unexpectedly.
    #[error("FFprobe stdout reader failed: {message}")]
    StdoutTask {
        /// Join diagnostic.
        message: String,
    },
    /// FFprobe exited unsuccessfully.
    #[error("FFprobe exited with code {code:?}: {stderr}")]
    Failed {
        /// Process exit code when the platform supplied one.
        code: Option<i32>,
        /// Bounded stderr tail.
        stderr: String,
    },
    /// Terminating the FFprobe process group failed after reaping was attempted.
    #[error("failed to terminate FFprobe process group: {source}")]
    Terminate {
        /// Operating-system error.
        #[source]
        source: io::Error,
    },
    /// Cancellation completed after FFprobe was reaped.
    #[error("FFprobe operation cancelled")]
    Cancelled,
    /// Standard output was not valid FFprobe JSON.
    #[error("invalid FFprobe JSON at line {line}, column {column}: {source}")]
    InvalidJson {
        /// One-based JSON error line.
        line: usize,
        /// One-based JSON error column.
        column: usize,
        /// Serde parser error.
        #[source]
        source: serde_json::Error,
    },
    /// A required FFprobe field was absent.
    #[error("missing required FFprobe field `{field}`{context}")]
    MissingField {
        /// JSON field name.
        field: &'static str,
        /// Optional stable stream context.
        context: String,
    },
    /// A FFprobe field had an invalid value.
    #[error("invalid FFprobe field `{field}`{context}: {reason}")]
    InvalidField {
        /// JSON field name.
        field: &'static str,
        /// Optional stable stream context.
        context: String,
        /// Bounded explanation without dumping the document.
        reason: String,
    },
    /// JSON values violated a domain invariant.
    #[error(transparent)]
    Model(#[from] ModelError),
}

/// External FFmpeg/FFprobe command adapter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FfmpegCliBackend {
    ffmpeg_path: PathBuf,
    ffprobe_path: PathBuf,
}

/// Explicit paths to a matching FFmpeg and FFprobe toolchain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FfmpegToolchainPaths {
    ffmpeg: PathBuf,
    ffprobe: PathBuf,
}

impl FfmpegToolchainPaths {
    /// Creates a named executable pair.
    #[must_use]
    pub fn new(ffmpeg: PathBuf, ffprobe: PathBuf) -> Self {
        Self { ffmpeg, ffprobe }
    }

    /// Returns the FFmpeg executable path.
    #[must_use]
    pub fn ffmpeg(&self) -> &Path {
        &self.ffmpeg
    }

    /// Returns the FFprobe executable path.
    #[must_use]
    pub fn ffprobe(&self) -> &Path {
        &self.ffprobe
    }
}

impl FfmpegCliBackend {
    /// Creates an adapter with explicit FFmpeg and FFprobe paths.
    #[must_use]
    pub fn new(paths: FfmpegToolchainPaths) -> Self {
        Self {
            ffmpeg_path: paths.ffmpeg,
            ffprobe_path: paths.ffprobe,
        }
    }

    /// Returns the configured FFmpeg executable.
    #[must_use]
    pub fn ffmpeg_path(&self) -> &Path {
        &self.ffmpeg_path
    }

    /// Returns the configured FFprobe executable.
    #[must_use]
    pub fn ffprobe_path(&self) -> &Path {
        &self.ffprobe_path
    }

    /// Builds FFprobe's argument protocol without executing a process.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::path::Path;
    /// use sonicmux_ffmpeg::FfmpegCliBackend;
    ///
    /// let arguments = FfmpegCliBackend::probe_arguments(Path::new("movie.mkv"));
    /// assert!(arguments.iter().any(|value| value == "-of"));
    /// ```
    #[must_use]
    pub fn probe_arguments(path: &Path) -> Vec<OsString> {
        [
            OsString::from("-v"),
            OsString::from("error"),
            OsString::from("-of"),
            OsString::from("json"),
            OsString::from("-show_streams"),
            OsString::from("-show_format"),
            OsString::from("-show_chapters"),
            path.as_os_str().to_owned(),
        ]
        .into_iter()
        .collect()
    }

    /// Probes one local media file and converts the result into domain data.
    ///
    /// # Errors
    ///
    /// Returns [`ProbeError`] when process execution, JSON parsing, or domain
    /// validation fails.
    pub async fn probe(&self, path: &Path) -> Result<MediaInfo, ProbeError> {
        self.probe_with_cancel(path, CancellationToken::new()).await
    }

    /// Probes one local media file with cooperative process cancellation.
    ///
    /// # Errors
    ///
    /// Returns [`ProbeError`] after the FFprobe process group has been reaped.
    pub async fn probe_with_cancel(
        &self,
        path: &Path,
        cancel: CancellationToken,
    ) -> Result<MediaInfo, ProbeError> {
        let absolute_path = if path.is_absolute() {
            path.to_path_buf()
        } else {
            env::current_dir()
                .map_err(|source| ProbeError::ResolveInput { source })?
                .join(path)
        };
        let mut command = Command::new(&self.ffprobe_path);
        command
            .args(Self::probe_arguments(&absolute_path))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        tracing::debug!(
            executable = %self.ffprobe_path.display(),
            input = %absolute_path.display(),
            "probing media"
        );
        let mut group = command.group();
        group.kill_on_drop(true);
        #[cfg(windows)]
        group.creation_flags(0x0800_0000);
        let mut child = group.spawn().map_err(|source| ProbeError::Spawn {
            executable: self.ffprobe_path.clone(),
            source,
        })?;
        let stdout = match child.inner().stdout.take() {
            Some(stdout) => stdout,
            None => {
                terminate_and_reap(&mut child).await?;
                return Err(ProbeError::MissingPipe { pipe: "stdout" });
            }
        };
        let stderr = match child.inner().stderr.take() {
            Some(stderr) => stderr,
            None => {
                terminate_and_reap(&mut child).await?;
                return Err(ProbeError::MissingPipe { pipe: "stderr" });
            }
        };
        let mut stdout_task = Some(tokio::spawn(async move {
            let mut stdout = stdout;
            read_bounded(&mut stdout, MAX_PROBE_STDOUT_BYTES).await
        }));
        let stderr_task = tokio::spawn(read_tail(stderr, MAX_PROBE_STDERR_BYTES));
        let mut stdout_result = None;
        let status = loop {
            if stdout_task
                .as_ref()
                .is_some_and(tokio::task::JoinHandle::is_finished)
            {
                let task = stdout_task.take().ok_or_else(|| ProbeError::StdoutTask {
                    message: "stdout reader handle disappeared".to_owned(),
                })?;
                match join_stdout(task.await) {
                    Ok(stdout) => stdout_result = Some(stdout),
                    Err(error) => {
                        terminate_and_reap(&mut child).await?;
                        let _ignored = stderr_task.await;
                        return Err(error);
                    }
                }
            }
            if let Some(status) = child
                .try_wait()
                .map_err(|source| ProbeError::Wait { source })?
            {
                break status;
            }
            tokio::select! {
                () = cancel.cancelled() => {
                    terminate_and_reap(&mut child).await?;
                    if let Some(task) = stdout_task {
                        let _ignored = task.await;
                    }
                    let _ignored = stderr_task.await;
                    return Err(ProbeError::Cancelled);
                }
                () = sleep(PROCESS_POLL_INTERVAL) => {}
            }
        };
        let stdout = match stdout_result {
            Some(stdout) => stdout,
            None => {
                let task = stdout_task.ok_or_else(|| ProbeError::StdoutTask {
                    message: "stdout reader handle disappeared".to_owned(),
                })?;
                join_stdout(task.await)?
            }
        };
        let stderr = join_stderr(stderr_task.await)?;
        if !status.success() {
            return Err(ProbeError::Failed {
                code: status.code(),
                stderr: String::from_utf8_lossy(&stderr).trim().to_owned(),
            });
        }
        parse_probe_output(absolute_path, &stdout)
    }
}

/// Parses an in-memory FFprobe JSON document into validated domain data.
///
/// This function performs no filesystem or process access.
///
/// # Errors
///
/// Returns [`ProbeError`] for malformed JSON, missing required fields, invalid
/// field values, or violated domain invariants.
pub fn parse_probe_output(path: PathBuf, json: &[u8]) -> Result<MediaInfo, ProbeError> {
    let document: RawProbeDocument =
        serde_json::from_slice(json).map_err(|source: serde_json::Error| {
            ProbeError::InvalidJson {
                line: source.line(),
                column: source.column(),
                source,
            }
        })?;
    convert_document(path, document)
}

#[derive(Debug, Deserialize)]
struct RawProbeDocument {
    #[serde(default)]
    streams: Vec<RawStream>,
    #[serde(default)]
    chapters: Vec<RawChapter>,
    format: Option<RawFormat>,
}

#[derive(Debug, Deserialize)]
struct RawStream {
    index: Option<u32>,
    codec_name: Option<String>,
    codec_type: Option<String>,
    profile: Option<String>,
    bit_rate: Option<Value>,
    sample_rate: Option<Value>,
    channels: Option<Value>,
    channel_layout: Option<String>,
    time_base: Option<String>,
    start_pts: Option<Value>,
    duration_ts: Option<Value>,
    #[serde(default)]
    tags: BTreeMap<String, Value>,
    #[serde(default)]
    disposition: BTreeMap<String, Value>,
}

#[derive(Debug, Deserialize)]
struct RawFormat {
    format_name: Option<String>,
    duration: Option<Value>,
    start_time: Option<Value>,
    bit_rate: Option<Value>,
    #[serde(default)]
    tags: BTreeMap<String, Value>,
}

#[derive(Debug, Deserialize)]
struct RawChapter {
    id: Option<Value>,
    time_base: Option<String>,
    start: Option<Value>,
    end: Option<Value>,
    #[serde(default)]
    tags: BTreeMap<String, Value>,
}

fn convert_document(path: PathBuf, document: RawProbeDocument) -> Result<MediaInfo, ProbeError> {
    let mut warnings = Vec::new();
    let raw_format = document.format.ok_or_else(|| missing("format", None))?;
    let format_names = raw_format
        .format_name
        .ok_or_else(|| missing("format.format_name", None))?
        .split(',')
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let format_metadata = convert_metadata(raw_format.tags, None, &mut warnings)?;
    let duration = parse_optional_decimal_micros(
        raw_format.duration.as_ref(),
        "format.duration",
        None,
        false,
        &mut warnings,
    )?
    .map(|value| DurationMicros::new(value as u64));
    let start_time = parse_optional_decimal_micros(
        raw_format.start_time.as_ref(),
        "format.start_time",
        None,
        true,
        &mut warnings,
    )?;
    let format_bitrate = parse_optional_u64(
        raw_format.bit_rate.as_ref(),
        "format.bit_rate",
        None,
        &mut warnings,
    )?
    .map(Bitrate::new)
    .transpose()?;
    let format = FormatInfo::new(format_names)?
        .with_duration(duration)
        .with_start_time(start_time)
        .with_bitrate(format_bitrate)
        .with_metadata(format_metadata);

    let streams = document
        .streams
        .into_iter()
        .map(|stream| convert_stream(stream, &mut warnings))
        .collect::<Result<Vec<_>, _>>()?;
    let chapters = document
        .chapters
        .into_iter()
        .map(|chapter| convert_chapter(chapter, &mut warnings))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(MediaInfo::new(path, format, streams, chapters)?.with_warnings(warnings))
}

fn convert_stream(
    raw: RawStream,
    warnings: &mut Vec<ProbeWarning>,
) -> Result<StreamInfo, ProbeError> {
    let index = StreamIndex::new(raw.index.ok_or_else(|| missing("streams[].index", None))?);
    let kind = raw
        .codec_type
        .as_deref()
        .ok_or_else(|| missing("streams[].codec_type", Some(index)))?;
    let codec_name = match raw.codec_name {
        Some(value) if !value.trim().is_empty() => value,
        _ => {
            warnings.push(ProbeWarning::MissingCodecName { stream: index });
            "unknown".to_owned()
        }
    };
    let bitrate = parse_optional_u64(
        raw.bit_rate.as_ref(),
        "streams[].bit_rate",
        Some(index),
        warnings,
    )?
    .map(Bitrate::new)
    .transpose()?;
    let time_base = raw
        .time_base
        .as_deref()
        .map(|value| parse_time_base(value, "streams[].time_base", Some(index)))
        .transpose()?;
    let start_pts = parse_optional_i64(
        raw.start_pts.as_ref(),
        "streams[].start_pts",
        Some(index),
        warnings,
    )?;
    let duration_ticks = parse_optional_i64(
        raw.duration_ts.as_ref(),
        "streams[].duration_ts",
        Some(index),
        warnings,
    )?;
    let timing = StreamTiming::new(
        start_pts
            .zip(time_base)
            .map(|(ticks, base)| MediaTimestamp::new(ticks, base)),
        duration_ticks,
    );
    let metadata = convert_metadata(raw.tags, Some(index), warnings)?;
    let dispositions = convert_dispositions(raw.disposition);
    let common = StreamCommon::new(index, codec_name.clone())?
        .with_profile(raw.profile.clone())
        .with_bitrate(bitrate)
        .with_timing(timing)
        .with_metadata(metadata)
        .with_dispositions(dispositions);

    Ok(match kind {
        "video" => StreamInfo::Video(VideoStream::new(common)),
        "audio" => {
            let channels_value = raw
                .channels
                .as_ref()
                .ok_or_else(|| missing("streams[].channels", Some(index)))?;
            let channel_number =
                parse_required_u64(channels_value, "streams[].channels", Some(index))?;
            let channel_number =
                u16::try_from(channel_number).map_err(|_| ProbeError::InvalidField {
                    field: "streams[].channels",
                    context: context(Some(index)),
                    reason: "does not fit u16".to_owned(),
                })?;
            let channels = Channels::new(ChannelCount::new(channel_number)?, raw.channel_layout);
            let sample_rate = parse_optional_u64(
                raw.sample_rate.as_ref(),
                "streams[].sample_rate",
                Some(index),
                warnings,
            )?
            .map(|value| {
                u32::try_from(value)
                    .map_err(|_| ProbeError::InvalidField {
                        field: "streams[].sample_rate",
                        context: context(Some(index)),
                        reason: "does not fit u32".to_owned(),
                    })
                    .and_then(|value| SampleRate::new(value).map_err(ProbeError::from))
            })
            .transpose()?;
            StreamInfo::Audio(AudioStream::new(
                common,
                AudioCodec::from_ffprobe(&codec_name, raw.profile.as_deref()),
                channels,
                sample_rate,
            ))
        }
        "subtitle" => StreamInfo::Subtitle(SubtitleStream::new(common)),
        "attachment" => StreamInfo::Attachment(AttachmentStream::new(
            common.clone(),
            common.metadata().get("filename").map(str::to_owned),
            common.metadata().get("mimetype").map(str::to_owned),
        )),
        "data" => StreamInfo::Data(DataStream::new(common)),
        other => StreamInfo::Unknown(UnknownStream::new(common, other.to_owned())),
    })
}

fn convert_chapter(
    raw: RawChapter,
    warnings: &mut Vec<ProbeWarning>,
) -> Result<Chapter, ProbeError> {
    let id = parse_required_i64(
        raw.id
            .as_ref()
            .ok_or_else(|| missing("chapters[].id", None))?,
        "chapters[].id",
        None,
    )?;
    let time_base = parse_time_base(
        raw.time_base
            .as_deref()
            .ok_or_else(|| missing("chapters[].time_base", None))?,
        "chapters[].time_base",
        None,
    )?;
    let start = parse_required_i64(
        raw.start
            .as_ref()
            .ok_or_else(|| missing("chapters[].start", None))?,
        "chapters[].start",
        None,
    )?;
    let end = parse_required_i64(
        raw.end
            .as_ref()
            .ok_or_else(|| missing("chapters[].end", None))?,
        "chapters[].end",
        None,
    )?;
    let metadata = convert_metadata(raw.tags, None, warnings)?;
    Chapter::new(id, time_base, start, end, metadata).map_err(ProbeError::from)
}

fn convert_metadata(
    raw: BTreeMap<String, Value>,
    stream: Option<StreamIndex>,
    warnings: &mut Vec<ProbeWarning>,
) -> Result<Metadata, ProbeError> {
    let mut values = BTreeMap::new();
    for (key, value) in raw {
        let converted = match value {
            Value::String(value) => Some(value),
            Value::Number(value) => Some(value.to_string()),
            Value::Bool(value) => Some(value.to_string()),
            Value::Null | Value::Array(_) | Value::Object(_) => None,
        };
        if let Some(value) = converted {
            values.insert(key, value);
        } else {
            warnings.push(ProbeWarning::UnsupportedMetadataValue { key, stream });
        }
    }
    Metadata::new(values).map_err(ProbeError::from)
}

fn convert_dispositions(raw: BTreeMap<String, Value>) -> Dispositions {
    Dispositions::from_flags(
        raw.into_iter()
            .map(|(name, value)| {
                let enabled = match value {
                    Value::Bool(value) => value,
                    Value::Number(value) => value.as_i64().is_some_and(|value| value != 0),
                    Value::String(value) => value.parse::<i64>().is_ok_and(|value| value != 0),
                    Value::Null | Value::Array(_) | Value::Object(_) => false,
                };
                (name, enabled)
            })
            .collect(),
    )
}

fn parse_time_base(
    value: &str,
    field: &'static str,
    stream: Option<StreamIndex>,
) -> Result<TimeBase, ProbeError> {
    let (numerator, denominator) = value
        .split_once('/')
        .ok_or_else(|| invalid(field, stream, "expected numerator/denominator"))?;
    let numerator = numerator
        .parse::<u32>()
        .map_err(|_| invalid(field, stream, "invalid numerator"))?;
    let denominator = denominator
        .parse::<u32>()
        .map_err(|_| invalid(field, stream, "invalid denominator"))?;
    TimeBase::new(numerator, denominator).map_err(ProbeError::from)
}

fn parse_optional_u64(
    value: Option<&Value>,
    field: &'static str,
    stream: Option<StreamIndex>,
    warnings: &mut Vec<ProbeWarning>,
) -> Result<Option<u64>, ProbeError> {
    let Some(value) = value else {
        return Ok(None);
    };
    match parse_u64(value) {
        Some(0) | None => {
            warnings.push(ProbeWarning::InvalidOptionalNumber {
                field: field.to_owned(),
                stream,
            });
            Ok(None)
        }
        Some(value) => Ok(Some(value)),
    }
}

fn parse_optional_i64(
    value: Option<&Value>,
    field: &'static str,
    stream: Option<StreamIndex>,
    warnings: &mut Vec<ProbeWarning>,
) -> Result<Option<i64>, ProbeError> {
    let Some(value) = value else {
        return Ok(None);
    };
    match parse_i64(value) {
        Some(value) => Ok(Some(value)),
        None => {
            warnings.push(ProbeWarning::InvalidOptionalNumber {
                field: field.to_owned(),
                stream,
            });
            Ok(None)
        }
    }
}

fn parse_required_u64(
    value: &Value,
    field: &'static str,
    stream: Option<StreamIndex>,
) -> Result<u64, ProbeError> {
    parse_u64(value).ok_or_else(|| invalid(field, stream, "expected a non-negative integer"))
}

fn parse_required_i64(
    value: &Value,
    field: &'static str,
    stream: Option<StreamIndex>,
) -> Result<i64, ProbeError> {
    parse_i64(value).ok_or_else(|| invalid(field, stream, "expected an integer"))
}

fn parse_u64(value: &Value) -> Option<u64> {
    match value {
        Value::Number(value) => value.as_u64(),
        Value::String(value) => value.parse().ok(),
        Value::Null | Value::Bool(_) | Value::Array(_) | Value::Object(_) => None,
    }
}

fn parse_i64(value: &Value) -> Option<i64> {
    match value {
        Value::Number(value) => value.as_i64(),
        Value::String(value) => value.parse().ok(),
        Value::Null | Value::Bool(_) | Value::Array(_) | Value::Object(_) => None,
    }
}

fn parse_optional_decimal_micros(
    value: Option<&Value>,
    field: &'static str,
    stream: Option<StreamIndex>,
    signed: bool,
    warnings: &mut Vec<ProbeWarning>,
) -> Result<Option<i64>, ProbeError> {
    let Some(value) = value else {
        return Ok(None);
    };
    let text = match value {
        Value::String(value) => value.as_str(),
        Value::Number(value) => {
            return parse_decimal_micros(&value.to_string(), signed)
                .map(Some)
                .ok_or_else(|| invalid(field, stream, "invalid decimal seconds"));
        }
        Value::Null | Value::Bool(_) | Value::Array(_) | Value::Object(_) => "",
    };
    if text.eq_ignore_ascii_case("n/a") || text.is_empty() {
        warnings.push(ProbeWarning::InvalidOptionalNumber {
            field: field.to_owned(),
            stream,
        });
        return Ok(None);
    }
    parse_decimal_micros(text, signed)
        .map(Some)
        .ok_or_else(|| invalid(field, stream, "invalid decimal seconds"))
}

fn parse_decimal_micros(value: &str, signed: bool) -> Option<i64> {
    let value = value.trim();
    let negative = value.starts_with('-');
    if negative && !signed {
        return None;
    }
    let unsigned = value.strip_prefix(['-', '+']).unwrap_or(value);
    let (seconds, fraction) = unsigned.split_once('.').unwrap_or((unsigned, ""));
    let seconds = seconds.parse::<i64>().ok()?;
    let mut micro_text: String = fraction.chars().take(6).collect();
    while micro_text.len() < 6 {
        micro_text.push('0');
    }
    let micros = if micro_text.is_empty() {
        0
    } else {
        micro_text.parse::<i64>().ok()?
    };
    let total = seconds.checked_mul(1_000_000)?.checked_add(micros)?;
    Some(if negative { -total } else { total })
}

async fn read_bounded<R>(reader: &mut R, limit: usize) -> Result<Vec<u8>, ProbeError>
where
    R: AsyncRead + Unpin,
{
    let mut output = Vec::new();
    let mut buffer = [0_u8; 8192];
    loop {
        let read = reader
            .read(&mut buffer)
            .await
            .map_err(|source| ProbeError::ReadOutput {
                pipe: "stdout",
                source,
            })?;
        if read == 0 {
            break;
        }
        if output.len().saturating_add(read) > limit {
            return Err(ProbeError::OutputTooLarge { limit_bytes: limit });
        }
        output.extend_from_slice(&buffer[..read]);
    }
    Ok(output)
}

async fn read_tail<R>(mut reader: R, limit: usize) -> Result<Vec<u8>, io::Error>
where
    R: AsyncRead + Unpin,
{
    let mut tail = Vec::new();
    let mut buffer = [0_u8; 4096];
    loop {
        let read = reader.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        if read >= limit {
            tail.clear();
            tail.extend_from_slice(&buffer[read - limit..read]);
            continue;
        }
        let overflow = tail.len().saturating_add(read).saturating_sub(limit);
        if overflow > 0 {
            tail.drain(..overflow);
        }
        tail.extend_from_slice(&buffer[..read]);
    }
    Ok(tail)
}

async fn terminate_and_reap(child: &mut AsyncGroupChild) -> Result<(), ProbeError> {
    if child
        .try_wait()
        .map_err(|source| ProbeError::Wait { source })?
        .is_some()
    {
        return Ok(());
    }
    let kill_error = child.start_kill().err();
    child
        .wait()
        .await
        .map_err(|source| ProbeError::Wait { source })?;
    if let Some(source) = kill_error {
        return Err(ProbeError::Terminate { source });
    }
    Ok(())
}

fn join_stdout(
    result: Result<Result<Vec<u8>, ProbeError>, JoinError>,
) -> Result<Vec<u8>, ProbeError> {
    result.map_err(|error| ProbeError::StdoutTask {
        message: error.to_string(),
    })?
}

fn join_stderr(
    result: Result<Result<Vec<u8>, io::Error>, JoinError>,
) -> Result<Vec<u8>, ProbeError> {
    match result {
        Ok(Ok(stderr)) => Ok(stderr),
        Ok(Err(error)) => Err(ProbeError::ReadOutput {
            pipe: "stderr",
            source: error,
        }),
        Err(error) => Err(ProbeError::StderrTask {
            message: error.to_string(),
        }),
    }
}

fn missing(field: &'static str, stream: Option<StreamIndex>) -> ProbeError {
    ProbeError::MissingField {
        field,
        context: context(stream),
    }
}

fn invalid(field: &'static str, stream: Option<StreamIndex>, reason: &str) -> ProbeError {
    ProbeError::InvalidField {
        field,
        context: context(stream),
        reason: reason.to_owned(),
    }
}

fn context(stream: Option<StreamIndex>) -> String {
    stream.map_or_else(String::new, |index| format!(" for stream {index}"))
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
mod tests {
    use std::{ffi::OsString, path::PathBuf};

    use insta::assert_debug_snapshot;
    use sonicmux_core::{AudioCodec, DtsProfile, StreamInfo};

    use super::{
        FfmpegCliBackend, ProbeError, parse_decimal_micros, parse_probe_output, read_bounded,
        read_tail,
    };

    const MIXED: &[u8] = include_bytes!("../tests/fixtures/mixed.json");
    const TRUEHD: &[u8] = include_bytes!("../tests/fixtures/truehd.json");
    const OPTIONALS: &[u8] = include_bytes!("../tests/fixtures/optional-fields.json");
    const DUPLICATE: &[u8] = include_bytes!("../tests/fixtures/duplicate-index.json");

    #[test]
    fn argument_protocol_uses_of_json() {
        assert_eq!(
            FfmpegCliBackend::probe_arguments(PathBuf::from("movie.mkv").as_path()),
            vec![
                OsString::from("-v"),
                OsString::from("error"),
                OsString::from("-of"),
                OsString::from("json"),
                OsString::from("-show_streams"),
                OsString::from("-show_format"),
                OsString::from("-show_chapters"),
                OsString::from("movie.mkv"),
            ]
        );
    }

    #[test]
    fn parses_mixed_streams_chapters_and_attachment() {
        let media = parse_probe_output(PathBuf::from("movie.mkv"), MIXED).expect("fixture parses");
        assert_eq!(media.streams().len(), 5);
        assert_eq!(media.chapters().len(), 1);
        assert!(media.format().is_matroska());
        let dts = media.audio_streams().next().expect("DTS stream exists");
        assert_eq!(dts.codec(), &AudioCodec::Dts(DtsProfile::HdMasterAudio));
        assert_eq!(
            dts.common()
                .metadata()
                .language()
                .map(|value| value.to_string()),
            Some("eng".to_owned())
        );
        let attachment = media
            .streams()
            .iter()
            .find_map(|stream| match stream {
                StreamInfo::Attachment(stream) => Some(stream),
                _ => None,
            })
            .expect("attachment exists");
        assert_eq!(attachment.filename(), Some("Font.ttf"));
        assert_eq!(attachment.mime_type(), Some("font/ttf"));
    }

    #[test]
    fn parses_truehd_codec() {
        let media = parse_probe_output(PathBuf::from("movie.mkv"), TRUEHD).expect("fixture parses");
        assert_eq!(
            media.audio_streams().next().map(|stream| stream.codec()),
            Some(&AudioCodec::TrueHd)
        );
    }

    #[test]
    fn missing_optional_numbers_become_warnings() {
        let media =
            parse_probe_output(PathBuf::from("movie.mkv"), OPTIONALS).expect("fixture parses");
        assert!(media.format().duration().is_none());
        assert!(media.warnings().len() >= 3);
    }

    #[test]
    fn duplicate_stream_indices_are_rejected() {
        assert!(matches!(
            parse_probe_output(PathBuf::from("movie.mkv"), DUPLICATE),
            Err(ProbeError::Model(_))
        ));
    }

    #[test]
    fn malformed_json_has_line_and_column() {
        let error = parse_probe_output(PathBuf::from("movie.mkv"), b"{\n invalid");
        assert!(matches!(
            error,
            Err(ProbeError::InvalidJson { line: 2, .. })
        ));
    }

    #[test]
    fn missing_audio_channels_is_a_contextual_error() {
        let json = br#"{"streams":[{"index":0,"codec_type":"audio","codec_name":"dts"}],"format":{"format_name":"matroska"}}"#;
        let error = parse_probe_output(PathBuf::from("movie.mkv"), json);
        assert!(matches!(
            error,
            Err(ProbeError::MissingField {
                field: "streams[].channels",
                ..
            })
        ));
    }

    #[test]
    fn decimal_seconds_are_parsed_without_float_rounding() {
        assert_eq!(parse_decimal_micros("12.3456789", false), Some(12_345_678));
        assert_eq!(parse_decimal_micros("-0.250000", true), Some(-250_000));
    }

    #[test]
    fn retains_unknown_disposition_flags() {
        let media = parse_probe_output(PathBuf::from("movie.mkv"), MIXED).expect("fixture parses");
        let audio = media.audio_streams().next().expect("audio exists");
        assert_eq!(
            audio.common().dispositions().flag("future_flag"),
            Some(true)
        );
    }

    #[test]
    fn snapshot_parsed_media() {
        let media = parse_probe_output(PathBuf::from("<INPUT>"), MIXED).expect("fixture parses");
        assert_debug_snapshot!("parsed_media", media);
    }

    #[test]
    fn snapshot_probe_arguments() {
        assert_debug_snapshot!(
            "probe_arguments",
            FfmpegCliBackend::probe_arguments(PathBuf::from("<INPUT>").as_path())
        );
    }

    #[tokio::test]
    async fn bounded_stdout_reader_rejects_oversized_json() {
        let mut input = &b"12345"[..];
        assert!(matches!(
            read_bounded(&mut input, 4).await,
            Err(ProbeError::OutputTooLarge { limit_bytes: 4 })
        ));
    }

    #[tokio::test]
    async fn stderr_reader_retains_only_the_tail() {
        let input = &b"0123456789"[..];
        let tail = read_tail(input, 4).await.expect("in-memory read succeeds");
        assert_eq!(tail, b"6789".to_vec());
    }
}
