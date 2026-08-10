//! Pure conversion and remux planning.

use std::{
    collections::BTreeMap,
    fmt,
    path::{Path, PathBuf},
};

use thiserror::Error;

use crate::{
    AudioStream, Bitrate, Chapter, Compatibility, CompatibilityPolicy, Dispositions, MediaInfo,
    Metadata, ModelError, PolicyError, StreamIndex, StreamInfo,
};

/// Error produced while validating a codec-specific target bitrate.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum TargetBitrateError {
    /// AC-3 accepts a discrete standardized bitrate set.
    #[error("{bitrate} bit/s is not a supported AC-3 bitrate")]
    UnsupportedAc3 {
        /// Rejected bitrate.
        bitrate: u64,
    },
    /// A bitrate is outside SonicMux's supported range for a codec.
    #[error("{codec} bitrate {bitrate} bit/s is outside {minimum}..={maximum}")]
    OutOfRange {
        /// Human-readable codec name.
        codec: &'static str,
        /// Rejected bitrate.
        bitrate: u64,
        /// Inclusive minimum.
        minimum: u64,
        /// Inclusive maximum.
        maximum: u64,
    },
}

/// Validated AC-3 target bitrate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Ac3Bitrate(u32);

impl Ac3Bitrate {
    /// Creates an AC-3 bitrate from the standardized discrete set.
    ///
    /// # Errors
    ///
    /// Returns [`TargetBitrateError::UnsupportedAc3`] for any unsupported value.
    ///
    /// # Examples
    ///
    /// ```
    /// use sonicmux_core::Ac3Bitrate;
    ///
    /// assert_eq!(Ac3Bitrate::new(640_000)?.get(), 640_000);
    /// assert!(Ac3Bitrate::new(639_000).is_err());
    /// # Ok::<(), sonicmux_core::TargetBitrateError>(())
    /// ```
    pub fn new(value: u64) -> Result<Self, TargetBitrateError> {
        const ALLOWED: &[u64] = &[
            32_000, 40_000, 48_000, 56_000, 64_000, 80_000, 96_000, 112_000, 128_000, 160_000,
            192_000, 224_000, 256_000, 320_000, 384_000, 448_000, 512_000, 576_000, 640_000,
        ];
        if !ALLOWED.contains(&value) {
            return Err(TargetBitrateError::UnsupportedAc3 { bitrate: value });
        }
        Ok(Self(value as u32))
    }

    /// Returns bits per second.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0 as u64
    }
}

impl Default for Ac3Bitrate {
    fn default() -> Self {
        Self(640_000)
    }
}

/// Validated E-AC-3 target bitrate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Eac3Bitrate(Bitrate);

impl Eac3Bitrate {
    /// Creates a SonicMux-supported E-AC-3 bitrate.
    ///
    /// # Errors
    ///
    /// Returns [`TargetBitrateError::OutOfRange`] outside 32–6144 kbit/s.
    pub fn new(value: u64) -> Result<Self, TargetBitrateError> {
        ranged_bitrate("E-AC-3", value, 32_000, 6_144_000).map(Self)
    }

    /// Returns bits per second.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0.get()
    }
}

/// Validated AAC target bitrate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AacBitrate(Bitrate);

impl AacBitrate {
    /// Creates a SonicMux-supported AAC bitrate.
    ///
    /// # Errors
    ///
    /// Returns [`TargetBitrateError::OutOfRange`] outside 8–1024 kbit/s.
    pub fn new(value: u64) -> Result<Self, TargetBitrateError> {
        ranged_bitrate("AAC", value, 8_000, 1_024_000).map(Self)
    }

    /// Returns bits per second.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0.get()
    }
}

/// Target channel-layout behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum TargetLayout {
    /// Retain the source layout up to 5.1 channels.
    KeepUpTo51,
    /// Downmix to stereo.
    Stereo,
    /// Produce 5.1 surround.
    Surround51,
}

impl fmt::Display for TargetLayout {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::KeepUpTo51 => "up to 5.1",
            Self::Stereo => "stereo",
            Self::Surround51 => "5.1",
        })
    }
}

/// Validated target audio codec, bitrate, and layout.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum AudioTarget {
    /// AC-3 target.
    Ac3 {
        /// Validated bitrate.
        bitrate: Ac3Bitrate,
        /// Target layout behavior.
        layout: TargetLayout,
    },
    /// E-AC-3 target.
    Eac3 {
        /// Validated bitrate.
        bitrate: Eac3Bitrate,
        /// Target layout behavior.
        layout: TargetLayout,
    },
    /// AAC target.
    Aac {
        /// Validated bitrate.
        bitrate: AacBitrate,
        /// Target layout behavior.
        layout: TargetLayout,
    },
}

impl AudioTarget {
    /// Returns a stable codec label.
    #[must_use]
    pub const fn codec_label(&self) -> &'static str {
        match self {
            Self::Ac3 { .. } => "AC-3",
            Self::Eac3 { .. } => "E-AC-3",
            Self::Aac { .. } => "AAC",
        }
    }

    /// Returns target layout behavior.
    #[must_use]
    pub const fn layout(&self) -> TargetLayout {
        match self {
            Self::Ac3 { layout, .. } | Self::Eac3 { layout, .. } | Self::Aac { layout, .. } => {
                *layout
            }
        }
    }
}

impl Default for AudioTarget {
    fn default() -> Self {
        Self::Ac3 {
            bitrate: Ac3Bitrate::default(),
            layout: TargetLayout::KeepUpTo51,
        }
    }
}

/// Audio output stream mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum OutputMode {
    /// Keep originals and append compatible derivatives.
    Add,
    /// Replace incompatible originals in their source positions.
    Replace,
    /// Omit all original audio and retain only new derivatives.
    OnlyNew,
}

/// Exact audio selection for remux-only planning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum AudioSelector {
    /// First compatible audio stream in source order.
    FirstCompatible,
    /// Exact source stream index.
    StreamIndex(StreamIndex),
}

/// User-requested high-level action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum RequestedAction {
    /// Convert incompatible audio according to the output mode.
    Convert,
    /// Copy every stream and change only audio default dispositions.
    RemuxOnly {
        /// Compatible stream selector.
        selection: AudioSelector,
    },
}

/// Fully merged input to the pure planner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanningPolicy {
    compatibility: CompatibilityPolicy,
    target: AudioTarget,
    output_mode: OutputMode,
    action: RequestedAction,
    output_path: PathBuf,
}

impl PlanningPolicy {
    /// Creates planning inputs.
    #[must_use]
    pub fn new(
        compatibility: CompatibilityPolicy,
        target: AudioTarget,
        output_mode: OutputMode,
        action: RequestedAction,
        output_path: PathBuf,
    ) -> Self {
        Self {
            compatibility,
            target,
            output_mode,
            action,
            output_path,
        }
    }

    /// Returns the compatibility policy.
    #[must_use]
    pub const fn compatibility(&self) -> &CompatibilityPolicy {
        &self.compatibility
    }

    /// Returns the selected target.
    #[must_use]
    pub const fn target(&self) -> &AudioTarget {
        &self.target
    }

    /// Returns the output mode.
    #[must_use]
    pub const fn output_mode(&self) -> OutputMode {
        self.output_mode
    }

    /// Returns the requested action.
    #[must_use]
    pub const fn action(&self) -> RequestedAction {
        self.action
    }

    /// Returns the already-resolved output path.
    #[must_use]
    pub fn output_path(&self) -> &Path {
        &self.output_path
    }
}

/// Planner failure.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum PlanError {
    /// The probe did not identify an MKV/Matroska input.
    #[error("input is not a Matroska .mkv file")]
    UnsupportedContainer,
    /// The input has no audio streams.
    #[error("input contains no audio streams")]
    NoAudioStreams,
    /// Direct input overwrite was requested outside the safe transaction layer.
    #[error("input and output paths are equal")]
    InputEqualsOutput,
    /// A codec cannot be planned safely under the active policy.
    #[error(transparent)]
    Policy(#[from] PolicyError),
    /// A generated metadata value violated a model invariant.
    #[error(transparent)]
    Model(#[from] ModelError),
    /// Remux selection did not identify a compatible stream.
    #[error("remux selection does not identify a compatible audio stream")]
    InvalidRemuxSelection,
    /// Remux was requested but the file contains no compatible audio.
    #[error("input contains no compatible audio stream for remux")]
    NoCompatibleRemuxCandidate,
    /// `only-new` would create a file with no audio.
    #[error("only-new has no incompatible audio to derive")]
    NothingToDo,
}

/// Reason an executable conversion was skipped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SkipReason {
    /// Every audio stream is already compatible.
    NothingToDo,
}

/// Result of pure planning.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum PlanOutcome {
    /// An executable stream plan.
    Execute(JobPlan),
    /// No output should be written.
    Skip(SkipReason),
}

/// Executable job kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum JobAction {
    /// At least one audio derivative is encoded.
    Transcode,
    /// Every stream is copied and only dispositions change.
    RemuxOnly,
}

/// Non-fatal fact that should be visible in dry-run and final reports.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum PlanWarning {
    /// The selected vendor/protocol profile is a conservative baseline.
    ConservativeCompatibilityProfile,
    /// FFprobe produced non-fatal warnings retained on the media model.
    ProbeWarningsPresent {
        /// Number of retained probe warnings.
        count: usize,
    },
}

/// Metadata assigned to an encoded derivative.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetadataPlan {
    metadata: Metadata,
}

impl MetadataPlan {
    /// Returns derivative metadata.
    #[must_use]
    pub const fn metadata(&self) -> &Metadata {
        &self.metadata
    }
}

/// One ordered output stream operation.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum OutputStreamPlan {
    /// Copy a source stream bitstream.
    Copy {
        /// Exact source stream.
        source: StreamIndex,
        /// Explicit output dispositions.
        dispositions: Dispositions,
    },
    /// Decode one source audio stream and encode its derivative.
    EncodeAudio {
        /// Exact source audio stream.
        source: StreamIndex,
        /// Validated audio target.
        target: AudioTarget,
        /// Explicit derivative metadata.
        metadata: MetadataPlan,
        /// Explicit output dispositions.
        dispositions: Dispositions,
    },
}

impl OutputStreamPlan {
    /// Returns the exact source stream index.
    #[must_use]
    pub const fn source(&self) -> StreamIndex {
        match self {
            Self::Copy { source, .. } | Self::EncodeAudio { source, .. } => *source,
        }
    }

    /// Returns whether the operation encodes audio.
    #[must_use]
    pub const fn is_encode(&self) -> bool {
        matches!(self, Self::EncodeAudio { .. })
    }

    /// Returns planned dispositions.
    #[must_use]
    pub const fn dispositions(&self) -> &Dispositions {
        match self {
            Self::Copy { dispositions, .. } | Self::EncodeAudio { dispositions, .. } => {
                dispositions
            }
        }
    }
}

/// Expected output codec operation.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ExpectedCodec {
    /// Source codec is copied.
    Copied(String),
    /// Audio is encoded to the target codec label.
    Encoded(&'static str),
}

/// One expected output stream used by M3 validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpectedStream {
    source: StreamIndex,
    codec: ExpectedCodec,
    metadata: Metadata,
    dispositions: Dispositions,
}

impl ExpectedStream {
    /// Returns the originating source stream.
    #[must_use]
    pub const fn source(&self) -> StreamIndex {
        self.source
    }

    /// Returns the expected codec operation.
    #[must_use]
    pub const fn codec(&self) -> &ExpectedCodec {
        &self.codec
    }

    /// Returns expected output metadata.
    #[must_use]
    pub const fn metadata(&self) -> &Metadata {
        &self.metadata
    }

    /// Returns expected output dispositions.
    #[must_use]
    pub const fn dispositions(&self) -> &Dispositions {
        &self.dispositions
    }
}

/// Structural postconditions for output validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputExpectations {
    streams: Vec<ExpectedStream>,
    chapters: Vec<Chapter>,
    global_metadata: Metadata,
}

impl OutputExpectations {
    /// Returns expected streams in output order.
    #[must_use]
    pub fn streams(&self) -> &[ExpectedStream] {
        &self.streams
    }

    /// Returns the expected chapter count.
    #[must_use]
    pub fn chapter_count(&self) -> usize {
        self.chapters.len()
    }

    /// Returns expected chapters including boundaries and metadata.
    #[must_use]
    pub fn chapters(&self) -> &[Chapter] {
        &self.chapters
    }

    /// Returns expected global metadata.
    #[must_use]
    pub const fn global_metadata(&self) -> &Metadata {
        &self.global_metadata
    }
}

/// Pure executable stream plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobPlan {
    input: PathBuf,
    output: PathBuf,
    action: JobAction,
    streams: Vec<OutputStreamPlan>,
    copy_chapters: bool,
    copy_global_metadata: bool,
    warnings: Vec<PlanWarning>,
    expected: OutputExpectations,
}

impl JobPlan {
    /// Returns the input path.
    #[must_use]
    pub fn input(&self) -> &Path {
        &self.input
    }

    /// Returns the output path.
    #[must_use]
    pub fn output(&self) -> &Path {
        &self.output
    }

    /// Returns the executable action.
    #[must_use]
    pub const fn action(&self) -> JobAction {
        self.action
    }

    /// Returns ordered stream operations.
    #[must_use]
    pub fn streams(&self) -> &[OutputStreamPlan] {
        &self.streams
    }

    /// Returns whether chapters are copied.
    #[must_use]
    pub const fn copy_chapters(&self) -> bool {
        self.copy_chapters
    }

    /// Returns whether global metadata is copied.
    #[must_use]
    pub const fn copy_global_metadata(&self) -> bool {
        self.copy_global_metadata
    }

    /// Returns stable non-fatal plan warnings.
    #[must_use]
    pub fn warnings(&self) -> &[PlanWarning] {
        &self.warnings
    }

    /// Returns output validation postconditions.
    #[must_use]
    pub const fn expected(&self) -> &OutputExpectations {
        &self.expected
    }
}

/// Builds a deterministic job plan without external I/O.
///
/// # Errors
///
/// Returns [`PlanError`] when the input or requested action cannot produce a safe
/// and valid output.
pub fn build(media: &MediaInfo, policy: &PlanningPolicy) -> Result<PlanOutcome, PlanError> {
    validate_media(media, policy)?;
    let classifications = classify_audio(media, policy.compatibility())?;
    match policy.action() {
        RequestedAction::Convert => build_convert(media, policy, &classifications),
        RequestedAction::RemuxOnly { selection } => {
            build_remux(media, policy, &classifications, selection)
        }
    }
}

fn validate_media(media: &MediaInfo, policy: &PlanningPolicy) -> Result<(), PlanError> {
    let mkv_extension = media
        .path()
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case("mkv"));
    if !mkv_extension || !media.format().is_matroska() {
        return Err(PlanError::UnsupportedContainer);
    }
    if media.audio_streams().next().is_none() {
        return Err(PlanError::NoAudioStreams);
    }
    if media.path() == policy.output_path() {
        return Err(PlanError::InputEqualsOutput);
    }
    Ok(())
}

fn classify_audio(
    media: &MediaInfo,
    policy: &CompatibilityPolicy,
) -> Result<BTreeMap<StreamIndex, Compatibility>, PlanError> {
    media
        .audio_streams()
        .map(|stream| {
            policy
                .classify(stream)
                .map(|compatibility| (stream.common().index(), compatibility))
                .map_err(PlanError::from)
        })
        .collect()
}

fn build_convert(
    media: &MediaInfo,
    policy: &PlanningPolicy,
    classifications: &BTreeMap<StreamIndex, Compatibility>,
) -> Result<PlanOutcome, PlanError> {
    let incompatible: Vec<&AudioStream> = media
        .audio_streams()
        .filter(|stream| {
            classifications
                .get(&stream.common().index())
                .is_some_and(|value| !value.is_compatible())
        })
        .collect();
    if incompatible.is_empty() {
        return match policy.output_mode() {
            OutputMode::OnlyNew => Err(PlanError::NothingToDo),
            OutputMode::Add | OutputMode::Replace => Ok(PlanOutcome::Skip(SkipReason::NothingToDo)),
        };
    }

    let mut operations = Vec::new();
    match policy.output_mode() {
        OutputMode::Add => {
            let any_source_default = media
                .audio_streams()
                .any(|stream| stream.common().dispositions().is_default());
            for stream in media.streams() {
                let mut dispositions = stream.common().dispositions().clone();
                if stream.as_audio().is_some()
                    && classifications
                        .get(&stream.index())
                        .is_some_and(|value| !value.is_compatible())
                    && dispositions.is_default()
                {
                    dispositions.set_default(false);
                }
                operations.push(copy_operation(stream, dispositions));
            }
            for (position, stream) in incompatible.iter().enumerate() {
                let mut dispositions = stream.common().dispositions().clone();
                if stream.common().dispositions().is_default()
                    || (!any_source_default && position == 0)
                {
                    dispositions.set_default(true);
                }
                operations.push(encode_operation(stream, policy.target(), dispositions)?);
            }
        }
        OutputMode::Replace => {
            for stream in media.streams() {
                let incompatible_audio = stream.as_audio().filter(|_| {
                    classifications
                        .get(&stream.index())
                        .is_some_and(|value| !value.is_compatible())
                });
                if let Some(audio) = incompatible_audio {
                    operations.push(encode_operation(
                        audio,
                        policy.target(),
                        audio.common().dispositions().clone(),
                    )?);
                } else {
                    operations.push(copy_operation(
                        stream,
                        stream.common().dispositions().clone(),
                    ));
                }
            }
        }
        OutputMode::OnlyNew => {
            for stream in media.streams() {
                if let Some(audio) = stream.as_audio() {
                    if classifications
                        .get(&stream.index())
                        .is_some_and(|value| !value.is_compatible())
                    {
                        operations.push(encode_operation(
                            audio,
                            policy.target(),
                            audio.common().dispositions().clone(),
                        )?);
                    }
                } else {
                    operations.push(copy_operation(
                        stream,
                        stream.common().dispositions().clone(),
                    ));
                }
            }
        }
    }

    Ok(PlanOutcome::Execute(job_plan(
        media,
        policy,
        JobAction::Transcode,
        operations,
    )))
}

fn build_remux(
    media: &MediaInfo,
    policy: &PlanningPolicy,
    classifications: &BTreeMap<StreamIndex, Compatibility>,
    selection: AudioSelector,
) -> Result<PlanOutcome, PlanError> {
    let compatible: Vec<StreamIndex> = media
        .audio_streams()
        .filter_map(|stream| {
            let index = stream.common().index();
            classifications
                .get(&index)
                .filter(|value| value.is_compatible())
                .map(|_| index)
        })
        .collect();
    let selected = match selection {
        AudioSelector::FirstCompatible => compatible
            .first()
            .copied()
            .ok_or(PlanError::NoCompatibleRemuxCandidate)?,
        AudioSelector::StreamIndex(index) if compatible.contains(&index) => index,
        AudioSelector::StreamIndex(_) => return Err(PlanError::InvalidRemuxSelection),
    };

    let operations = media
        .streams()
        .iter()
        .map(|stream| {
            let mut dispositions = stream.common().dispositions().clone();
            if stream.as_audio().is_some() {
                dispositions.set_default(stream.index() == selected);
            }
            copy_operation(stream, dispositions)
        })
        .collect();

    Ok(PlanOutcome::Execute(job_plan(
        media,
        policy,
        JobAction::RemuxOnly,
        operations,
    )))
}

fn copy_operation(stream: &StreamInfo, dispositions: Dispositions) -> OutputStreamPlan {
    OutputStreamPlan::Copy {
        source: stream.index(),
        dispositions,
    }
}

fn encode_operation(
    stream: &AudioStream,
    target: &AudioTarget,
    dispositions: Dispositions,
) -> Result<OutputStreamPlan, ModelError> {
    let mut metadata = stream.common().metadata().clone();
    let suffix = format!(
        "{} {}",
        target.codec_label(),
        effective_layout_label(target, stream)
    );
    let title = match metadata.title() {
        Some(title) => format!("{title} [{suffix}]"),
        None => suffix,
    };
    let _previous = metadata.insert("title", title)?;
    Ok(OutputStreamPlan::EncodeAudio {
        source: stream.common().index(),
        target: target.clone(),
        metadata: MetadataPlan { metadata },
        dispositions,
    })
}

fn effective_layout_label(target: &AudioTarget, stream: &AudioStream) -> &'static str {
    match target.layout() {
        TargetLayout::Stereo => "stereo",
        TargetLayout::Surround51 => "5.1",
        TargetLayout::KeepUpTo51 => match stream.channels().count().get() {
            1 => "mono",
            2 => "stereo",
            3 => "3.0",
            4 => "4.0",
            5 => "5.0",
            _ => "5.1",
        },
    }
}

fn job_plan(
    media: &MediaInfo,
    policy: &PlanningPolicy,
    action: JobAction,
    streams: Vec<OutputStreamPlan>,
) -> JobPlan {
    let expected_streams = streams
        .iter()
        .map(|operation| match operation {
            OutputStreamPlan::Copy {
                source,
                dispositions,
            } => {
                let source_stream = media
                    .streams()
                    .iter()
                    .find(|stream| stream.index() == *source);
                let codec = source_stream.map_or_else(
                    || "unknown".to_owned(),
                    |stream| stream.common().codec_name().to_owned(),
                );
                let metadata = source_stream.map_or_else(Metadata::default, |stream| {
                    stream.common().metadata().clone()
                });
                ExpectedStream {
                    source: *source,
                    codec: ExpectedCodec::Copied(codec),
                    metadata,
                    dispositions: dispositions.clone(),
                }
            }
            OutputStreamPlan::EncodeAudio {
                source,
                target,
                metadata,
                dispositions,
            } => ExpectedStream {
                source: *source,
                codec: ExpectedCodec::Encoded(target.codec_label()),
                metadata: metadata.metadata().clone(),
                dispositions: dispositions.clone(),
            },
        })
        .collect();
    let mut warnings = Vec::new();
    if policy.compatibility().is_conservative_baseline() {
        warnings.push(PlanWarning::ConservativeCompatibilityProfile);
    }
    if !media.warnings().is_empty() {
        warnings.push(PlanWarning::ProbeWarningsPresent {
            count: media.warnings().len(),
        });
    }
    JobPlan {
        input: media.path().to_path_buf(),
        output: policy.output_path().to_path_buf(),
        action,
        streams,
        copy_chapters: true,
        copy_global_metadata: true,
        warnings,
        expected: OutputExpectations {
            streams: expected_streams,
            chapters: media.chapters().to_vec(),
            global_metadata: media.format().metadata().clone(),
        },
    }
}

fn ranged_bitrate(
    codec: &'static str,
    value: u64,
    minimum: u64,
    maximum: u64,
) -> Result<Bitrate, TargetBitrateError> {
    if !(minimum..=maximum).contains(&value) {
        return Err(TargetBitrateError::OutOfRange {
            codec,
            bitrate: value,
            minimum,
            maximum,
        });
    }
    Bitrate::new(value).map_err(|_| TargetBitrateError::OutOfRange {
        codec,
        bitrate: value,
        minimum,
        maximum,
    })
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
mod tests {
    use std::{collections::BTreeMap, path::PathBuf};

    use insta::assert_debug_snapshot;
    use proptest::prelude::*;

    use super::{
        Ac3Bitrate, AudioSelector, AudioTarget, JobAction, OutputMode, OutputStreamPlan, PlanError,
        PlanOutcome, PlanningPolicy, RequestedAction, SkipReason, TargetLayout, build,
    };
    use crate::{
        AttachmentStream, AudioCodec, AudioStream, ChannelCount, Channels, CompatibilityPolicy,
        DataStream, Dispositions, DtsProfile, FormatInfo, MediaInfo, Metadata, ProfileName,
        StreamCommon, StreamIndex, StreamInfo, SubtitleStream, TimeBase, VideoStream,
    };

    #[derive(Clone)]
    struct AudioSpec {
        codec: AudioCodec,
        channels: u16,
        default: bool,
        language: Option<&'static str>,
        title: Option<&'static str>,
    }

    fn dts(default: bool) -> AudioSpec {
        AudioSpec {
            codec: AudioCodec::Dts(DtsProfile::Core),
            channels: 6,
            default,
            language: Some("eng"),
            title: Some("Main"),
        }
    }

    fn ac3(default: bool) -> AudioSpec {
        AudioSpec {
            codec: AudioCodec::Ac3,
            channels: 6,
            default,
            language: Some("eng"),
            title: Some("Compatible"),
        }
    }

    fn audio_stream(index: u32, spec: &AudioSpec) -> StreamInfo {
        let mut flags = BTreeMap::new();
        flags.insert("default".to_owned(), spec.default);
        flags.insert("forced".to_owned(), true);
        let mut metadata = Metadata::default();
        if let Some(language) = spec.language {
            metadata
                .insert("language", language)
                .expect("test language is valid");
        }
        if let Some(title) = spec.title {
            metadata
                .insert("title", title)
                .expect("test title is valid");
        }
        let codec_name = match spec.codec {
            AudioCodec::Ac3 => "ac3",
            AudioCodec::Aac => "aac",
            AudioCodec::Mp3 => "mp3",
            AudioCodec::TrueHd => "truehd",
            AudioCodec::Flac => "flac",
            AudioCodec::Dts(_) => "dts",
            _ => "other",
        };
        let common = StreamCommon::new(StreamIndex::new(index), codec_name)
            .expect("test codec is valid")
            .with_metadata(metadata)
            .with_dispositions(Dispositions::from_flags(flags));
        StreamInfo::Audio(AudioStream::new(
            common,
            spec.codec.clone(),
            Channels::new(
                ChannelCount::new(spec.channels).expect("test channels are valid"),
                None,
            ),
            None,
        ))
    }

    fn common(index: u32, codec: &str) -> StreamCommon {
        StreamCommon::new(StreamIndex::new(index), codec).expect("test codec is valid")
    }

    fn media(audio: &[AudioSpec]) -> MediaInfo {
        let mut streams = vec![StreamInfo::Video(VideoStream::new(common(0, "hevc")))];
        streams.extend(
            audio
                .iter()
                .enumerate()
                .map(|(position, spec)| audio_stream(position as u32 + 1, spec)),
        );
        MediaInfo::new(
            PathBuf::from("movie.mkv"),
            FormatInfo::new(vec!["matroska".to_owned()]).expect("format is valid"),
            streams,
            Vec::new(),
        )
        .expect("media is valid")
    }

    fn policy(mode: OutputMode, action: RequestedAction) -> PlanningPolicy {
        PlanningPolicy::new(
            CompatibilityPolicy::for_profile(ProfileName::GenericTv),
            AudioTarget::Ac3 {
                bitrate: Ac3Bitrate::new(640_000).expect("default bitrate is valid"),
                layout: TargetLayout::KeepUpTo51,
            },
            mode,
            action,
            PathBuf::from("movie.sonicmux.mkv"),
        )
    }

    fn execute(outcome: PlanOutcome) -> super::JobPlan {
        match outcome {
            PlanOutcome::Execute(plan) => plan,
            PlanOutcome::Skip(reason) => panic!("expected execute, got {reason:?}"),
        }
    }

    #[test]
    fn add_appends_derivative_and_copies_source() {
        let plan = execute(
            build(
                &media(&[dts(true)]),
                &policy(OutputMode::Add, RequestedAction::Convert),
            )
            .expect("plan succeeds"),
        );
        assert_eq!(plan.streams().len(), 3);
        assert!(!plan.streams()[1].is_encode());
        assert!(plan.streams()[2].is_encode());
    }

    #[test]
    fn add_orders_multiple_derivatives_by_source_audio() {
        let plan = execute(
            build(
                &media(&[dts(false), dts(false)]),
                &policy(OutputMode::Add, RequestedAction::Convert),
            )
            .expect("plan succeeds"),
        );
        let encoded: Vec<_> = plan
            .streams()
            .iter()
            .filter(|value| value.is_encode())
            .map(OutputStreamPlan::source)
            .collect();
        assert_eq!(encoded, [StreamIndex::new(1), StreamIndex::new(2)]);
    }

    #[test]
    fn add_with_only_compatible_audio_skips() {
        assert_eq!(
            build(
                &media(&[ac3(true)]),
                &policy(OutputMode::Add, RequestedAction::Convert)
            ),
            Ok(PlanOutcome::Skip(SkipReason::NothingToDo))
        );
    }

    #[test]
    fn add_transfers_default_from_incompatible_source() {
        let plan = execute(
            build(
                &media(&[dts(true)]),
                &policy(OutputMode::Add, RequestedAction::Convert),
            )
            .expect("plan succeeds"),
        );
        assert!(!plan.streams()[1].dispositions().is_default());
        assert!(plan.streams()[2].dispositions().is_default());
        assert_eq!(plan.streams()[1].dispositions().flag("forced"), Some(true));
    }

    #[test]
    fn add_selects_first_derivative_when_no_source_is_default() {
        let plan = execute(
            build(
                &media(&[dts(false), dts(false)]),
                &policy(OutputMode::Add, RequestedAction::Convert),
            )
            .expect("plan succeeds"),
        );
        let encoded: Vec<_> = plan
            .streams()
            .iter()
            .filter(|value| value.is_encode())
            .collect();
        assert!(encoded[0].dispositions().is_default());
        assert!(!encoded[1].dispositions().is_default());
    }

    #[test]
    fn add_preserves_existing_compatible_default() {
        let plan = execute(
            build(
                &media(&[ac3(true), dts(false)]),
                &policy(OutputMode::Add, RequestedAction::Convert),
            )
            .expect("plan succeeds"),
        );
        assert!(plan.streams()[1].dispositions().is_default());
        assert!(
            !plan
                .streams()
                .last()
                .expect("derivative exists")
                .dispositions()
                .is_default()
        );
    }

    #[test]
    fn replace_substitutes_incompatible_stream_in_position() {
        let plan = execute(
            build(
                &media(&[dts(true)]),
                &policy(OutputMode::Replace, RequestedAction::Convert),
            )
            .expect("plan succeeds"),
        );
        assert_eq!(plan.streams().len(), 2);
        assert!(plan.streams()[1].is_encode());
        assert_eq!(plan.streams()[1].source(), StreamIndex::new(1));
    }

    #[test]
    fn replace_copies_compatible_audio_in_mixed_input() {
        let plan = execute(
            build(
                &media(&[ac3(true), dts(false)]),
                &policy(OutputMode::Replace, RequestedAction::Convert),
            )
            .expect("plan succeeds"),
        );
        assert!(!plan.streams()[1].is_encode());
        assert!(plan.streams()[2].is_encode());
    }

    #[test]
    fn only_new_omits_all_original_audio() {
        let plan = execute(
            build(
                &media(&[ac3(true), dts(false)]),
                &policy(OutputMode::OnlyNew, RequestedAction::Convert),
            )
            .expect("plan succeeds"),
        );
        assert_eq!(plan.streams().len(), 2);
        assert_eq!(plan.streams()[1].source(), StreamIndex::new(2));
        assert!(plan.streams()[1].is_encode());
    }

    #[test]
    fn only_new_errors_without_derivative() {
        assert_eq!(
            build(
                &media(&[ac3(true)]),
                &policy(OutputMode::OnlyNew, RequestedAction::Convert)
            ),
            Err(PlanError::NothingToDo)
        );
    }

    #[test]
    fn remux_chooses_first_compatible_stream() {
        let action = RequestedAction::RemuxOnly {
            selection: AudioSelector::FirstCompatible,
        };
        let plan = execute(
            build(
                &media(&[dts(true), ac3(false)]),
                &policy(OutputMode::Add, action),
            )
            .expect("plan succeeds"),
        );
        assert_eq!(plan.action(), JobAction::RemuxOnly);
        assert!(!plan.streams()[1].dispositions().is_default());
        assert!(plan.streams()[2].dispositions().is_default());
    }

    #[test]
    fn remux_honors_exact_compatible_index() {
        let action = RequestedAction::RemuxOnly {
            selection: AudioSelector::StreamIndex(StreamIndex::new(2)),
        };
        let plan = execute(
            build(
                &media(&[ac3(true), ac3(false)]),
                &policy(OutputMode::Add, action),
            )
            .expect("plan succeeds"),
        );
        assert!(!plan.streams()[1].dispositions().is_default());
        assert!(plan.streams()[2].dispositions().is_default());
    }

    #[test]
    fn remux_rejects_incompatible_selection() {
        let action = RequestedAction::RemuxOnly {
            selection: AudioSelector::StreamIndex(StreamIndex::new(1)),
        };
        assert_eq!(
            build(
                &media(&[dts(true), ac3(false)]),
                &policy(OutputMode::Add, action)
            ),
            Err(PlanError::InvalidRemuxSelection)
        );
    }

    #[test]
    fn remux_rejects_missing_selection() {
        let action = RequestedAction::RemuxOnly {
            selection: AudioSelector::StreamIndex(StreamIndex::new(99)),
        };
        assert_eq!(
            build(&media(&[ac3(true)]), &policy(OutputMode::Add, action)),
            Err(PlanError::InvalidRemuxSelection)
        );
    }

    #[test]
    fn remux_emits_no_encoder_operation() {
        let action = RequestedAction::RemuxOnly {
            selection: AudioSelector::FirstCompatible,
        };
        let plan = execute(
            build(
                &media(&[ac3(true), dts(false)]),
                &policy(OutputMode::Add, action),
            )
            .expect("plan succeeds"),
        );
        assert!(plan.streams().iter().all(|value| !value.is_encode()));
    }

    #[test]
    fn video_is_always_copied() {
        let plan = execute(
            build(
                &media(&[dts(true)]),
                &policy(OutputMode::Replace, RequestedAction::Convert),
            )
            .expect("plan succeeds"),
        );
        assert_eq!(plan.streams()[0].source(), StreamIndex::new(0));
        assert!(!plan.streams()[0].is_encode());
    }

    #[test]
    fn subtitle_attachment_and_data_are_copied() {
        let streams = vec![
            StreamInfo::Video(VideoStream::new(common(0, "hevc"))),
            audio_stream(1, &dts(true)),
            StreamInfo::Subtitle(SubtitleStream::new(common(2, "ass"))),
            StreamInfo::Attachment(AttachmentStream::new(
                common(3, "ttf"),
                Some("font.ttf".to_owned()),
                Some("font/ttf".to_owned()),
            )),
            StreamInfo::Data(DataStream::new(common(4, "bin_data"))),
        ];
        let input = MediaInfo::new(
            PathBuf::from("movie.mkv"),
            FormatInfo::new(vec!["matroska".to_owned()]).expect("format"),
            streams,
            Vec::new(),
        )
        .expect("media");
        let plan = execute(
            build(
                &input,
                &policy(OutputMode::Replace, RequestedAction::Convert),
            )
            .expect("plan succeeds"),
        );
        assert!(
            plan.streams()
                .iter()
                .filter(|operation| [2, 3, 4].contains(&operation.source().get()))
                .all(|operation| !operation.is_encode())
        );
    }

    #[test]
    fn chapters_and_global_metadata_are_copied() {
        let source = media(&[dts(true)]);
        let chapter = crate::Chapter::new(
            0,
            TimeBase::new(1, 1_000).expect("time base is valid"),
            0,
            5_000,
            Metadata::default(),
        )
        .expect("chapter is valid");
        let source = MediaInfo::new(
            source.path().to_path_buf(),
            source.format().clone(),
            source.streams().to_vec(),
            vec![chapter.clone()],
        )
        .expect("media is valid");
        let plan = execute(
            build(
                &source,
                &policy(OutputMode::Replace, RequestedAction::Convert),
            )
            .expect("plan succeeds"),
        );
        assert!(plan.copy_chapters());
        assert!(plan.copy_global_metadata());
        assert_eq!(plan.expected().chapters(), [chapter]);
        assert_eq!(
            plan.expected().global_metadata(),
            source.format().metadata()
        );
    }

    #[test]
    fn language_and_title_are_inherited() {
        let plan = execute(
            build(
                &media(&[dts(true)]),
                &policy(OutputMode::Replace, RequestedAction::Convert),
            )
            .expect("plan succeeds"),
        );
        let OutputStreamPlan::EncodeAudio { metadata, .. } = &plan.streams()[1] else {
            panic!("expected derivative")
        };
        assert_eq!(metadata.metadata().get("language"), Some("eng"));
        assert_eq!(metadata.metadata().get("title"), Some("Main [AC-3 5.1]"));
    }

    #[test]
    fn title_without_source_title_is_deterministic() {
        let mut spec = dts(true);
        spec.title = None;
        let plan = execute(
            build(
                &media(&[spec]),
                &policy(OutputMode::Replace, RequestedAction::Convert),
            )
            .expect("plan succeeds"),
        );
        let OutputStreamPlan::EncodeAudio { metadata, .. } = &plan.streams()[1] else {
            panic!("expected derivative")
        };
        assert_eq!(metadata.metadata().get("title"), Some("AC-3 5.1"));
    }

    #[test]
    fn no_audio_input_is_rejected() {
        let input = MediaInfo::new(
            PathBuf::from("movie.mkv"),
            FormatInfo::new(vec!["matroska".to_owned()]).expect("format"),
            vec![StreamInfo::Video(VideoStream::new(common(0, "hevc")))],
            Vec::new(),
        )
        .expect("media");
        assert_eq!(
            build(&input, &policy(OutputMode::Add, RequestedAction::Convert)),
            Err(PlanError::NoAudioStreams)
        );
    }

    #[test]
    fn non_matroska_input_is_rejected() {
        let mut input = media(&[dts(true)]);
        input = MediaInfo::new(
            PathBuf::from("movie.mp4"),
            FormatInfo::new(vec!["mov".to_owned()]).expect("format"),
            input.streams().to_vec(),
            Vec::new(),
        )
        .expect("media");
        assert_eq!(
            build(&input, &policy(OutputMode::Add, RequestedAction::Convert)),
            Err(PlanError::UnsupportedContainer)
        );
    }

    #[test]
    fn equal_input_output_paths_are_rejected() {
        let mut policy = policy(OutputMode::Add, RequestedAction::Convert);
        policy.output_path = PathBuf::from("movie.mkv");
        assert_eq!(
            build(&media(&[dts(true)]), &policy),
            Err(PlanError::InputEqualsOutput)
        );
    }

    #[test]
    fn unknown_codec_without_fallback_is_rejected() {
        let spec = AudioSpec {
            codec: AudioCodec::Other("future".to_owned()),
            channels: 2,
            default: true,
            language: None,
            title: None,
        };
        assert!(matches!(
            build(
                &media(&[spec]),
                &policy(OutputMode::Add, RequestedAction::Convert)
            ),
            Err(PlanError::Policy(_))
        ));
    }

    #[test]
    fn equal_inputs_produce_equal_plans() {
        let input = media(&[dts(true)]);
        let policy = policy(OutputMode::Add, RequestedAction::Convert);
        assert_eq!(build(&input, &policy), build(&input, &policy));
    }

    #[test]
    fn target_bitrate_rejects_nonstandard_ac3_value() {
        assert!(Ac3Bitrate::new(639_000).is_err());
    }

    #[test]
    fn snapshot_add_plan() {
        assert_debug_snapshot!(
            "add_plan",
            build(
                &media(&[dts(true)]),
                &policy(OutputMode::Add, RequestedAction::Convert)
            )
        );
    }

    #[test]
    fn snapshot_replace_plan() {
        assert_debug_snapshot!(
            "replace_plan",
            build(
                &media(&[ac3(true), dts(false)]),
                &policy(OutputMode::Replace, RequestedAction::Convert)
            )
        );
    }

    #[test]
    fn snapshot_only_new_plan() {
        assert_debug_snapshot!(
            "only_new_plan",
            build(
                &media(&[ac3(true), dts(false)]),
                &policy(OutputMode::OnlyNew, RequestedAction::Convert)
            )
        );
    }

    #[test]
    fn snapshot_remux_plan() {
        let action = RequestedAction::RemuxOnly {
            selection: AudioSelector::FirstCompatible,
        };
        assert_debug_snapshot!(
            "remux_plan",
            build(
                &media(&[dts(true), ac3(false)]),
                &policy(OutputMode::Add, action)
            )
        );
    }

    #[test]
    fn snapshot_mixed_dispositions_and_metadata() {
        assert_debug_snapshot!(
            "mixed_metadata",
            build(
                &media(&[ac3(true), dts(false), dts(false)]),
                &policy(OutputMode::Add, RequestedAction::Convert)
            )
        );
    }

    #[test]
    fn snapshot_typed_failure() {
        assert_debug_snapshot!(
            "typed_failure",
            build(
                &media(&[ac3(true)]),
                &policy(OutputMode::OnlyNew, RequestedAction::Convert)
            )
        );
    }

    proptest! {
        #[test]
        fn replace_never_changes_source_origin_count(incompatible in prop::collection::vec(any::<bool>(), 1..12)) {
            let specs: Vec<_> = incompatible.iter().map(|value| if *value { dts(false) } else { ac3(false) }).collect();
            let outcome = build(&media(&specs), &policy(OutputMode::Replace, RequestedAction::Convert)).expect("planning succeeds");
            if incompatible.iter().any(|value| *value) {
                let plan = execute(outcome);
                prop_assert_eq!(plan.streams().len(), specs.len() + 1);
            } else {
                prop_assert_eq!(outcome, PlanOutcome::Skip(SkipReason::NothingToDo));
            }
        }

        #[test]
        fn remux_never_encodes(extra_dts in 0usize..12) {
            let mut specs = vec![ac3(true)];
            specs.extend((0..extra_dts).map(|_| dts(false)));
            let action = RequestedAction::RemuxOnly { selection: AudioSelector::FirstCompatible };
            let plan = execute(build(&media(&specs), &policy(OutputMode::Add, action)).expect("planning succeeds"));
            prop_assert!(plan.streams().iter().all(|operation| !operation.is_encode()));
        }

        #[test]
        fn add_duplicates_only_incompatible_audio_sources(incompatible in prop::collection::vec(any::<bool>(), 1..12)) {
            let specs: Vec<_> = incompatible.iter().map(|value| if *value { dts(false) } else { ac3(false) }).collect();
            let outcome = build(&media(&specs), &policy(OutputMode::Add, RequestedAction::Convert)).expect("planning succeeds");
            if incompatible.iter().any(|value| *value) {
                let plan = execute(outcome);
                for (position, expected_duplicate) in incompatible.iter().enumerate() {
                    let source = StreamIndex::new(position as u32 + 1);
                    let count = plan.streams().iter().filter(|operation| operation.source() == source).count();
                    prop_assert_eq!(count, if *expected_duplicate { 2 } else { 1 });
                }
            }
        }
    }
}
