//! Validated media-domain types independent from FFprobe's JSON representation.

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    num::{NonZeroU16, NonZeroU32, NonZeroU64},
    path::{Path, PathBuf},
};

use thiserror::Error;

/// An error encountered while constructing validated media-domain data.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ModelError {
    /// A value that must be positive was zero.
    #[error("{field} must be greater than zero")]
    ZeroValue {
        /// Name of the invalid field.
        field: &'static str,
    },
    /// A string value was empty or contained a NUL character.
    #[error("invalid {field}: {reason}")]
    InvalidText {
        /// Name of the invalid field.
        field: &'static str,
        /// Stable explanation of the violation.
        reason: &'static str,
    },
    /// A rational time base was invalid.
    #[error("time base numerator and denominator must be greater than zero")]
    InvalidTimeBase,
    /// Media information contained no streams.
    #[error("media information contains no streams")]
    NoStreams,
    /// Two streams used the same source index.
    #[error("duplicate stream index {index}")]
    DuplicateStreamIndex {
        /// Duplicated index.
        index: StreamIndex,
    },
    /// Format identity did not contain any usable name.
    #[error("media format has no names")]
    MissingFormatName,
    /// A chapter ended before it started.
    #[error("chapter {id} ends before it starts")]
    InvalidChapterRange {
        /// Chapter identifier.
        id: i64,
    },
}

/// Source stream index as reported by the demuxer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StreamIndex(u32);

impl StreamIndex {
    /// Creates a stream index.
    #[must_use]
    pub const fn new(value: u32) -> Self {
        Self(value)
    }

    /// Returns the numeric stream index.
    #[must_use]
    pub const fn get(self) -> u32 {
        self.0
    }
}

impl fmt::Display for StreamIndex {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// A positive bitrate measured in bits per second.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Bitrate(NonZeroU64);

impl Bitrate {
    /// Creates a positive bitrate.
    ///
    /// # Errors
    ///
    /// Returns [`ModelError::ZeroValue`] when `value` is zero.
    ///
    /// # Examples
    ///
    /// ```
    /// use sonicmux_core::Bitrate;
    ///
    /// let bitrate = Bitrate::new(640_000)?;
    /// assert_eq!(bitrate.get(), 640_000);
    /// # Ok::<(), sonicmux_core::ModelError>(())
    /// ```
    pub const fn new(value: u64) -> Result<Self, ModelError> {
        match NonZeroU64::new(value) {
            Some(value) => Ok(Self(value)),
            None => Err(ModelError::ZeroValue { field: "bitrate" }),
        }
    }

    /// Returns bits per second.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0.get()
    }
}

impl fmt::Display for Bitrate {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} bit/s", self.get())
    }
}

/// A positive number of audio channels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ChannelCount(NonZeroU16);

impl ChannelCount {
    /// Creates a positive channel count.
    ///
    /// # Errors
    ///
    /// Returns [`ModelError::ZeroValue`] when `value` is zero.
    pub const fn new(value: u16) -> Result<Self, ModelError> {
        match NonZeroU16::new(value) {
            Some(value) => Ok(Self(value)),
            None => Err(ModelError::ZeroValue { field: "channels" }),
        }
    }

    /// Returns the channel count.
    #[must_use]
    pub const fn get(self) -> u16 {
        self.0.get()
    }
}

impl fmt::Display for ChannelCount {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.get().fmt(formatter)
    }
}

/// A positive audio sample rate measured in hertz.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SampleRate(NonZeroU32);

impl SampleRate {
    /// Creates a positive sample rate.
    ///
    /// # Errors
    ///
    /// Returns [`ModelError::ZeroValue`] when `value` is zero.
    pub const fn new(value: u32) -> Result<Self, ModelError> {
        match NonZeroU32::new(value) {
            Some(value) => Ok(Self(value)),
            None => Err(ModelError::ZeroValue {
                field: "sample rate",
            }),
        }
    }

    /// Returns hertz.
    #[must_use]
    pub const fn get(self) -> u32 {
        self.0.get()
    }
}

/// A retained, non-empty source language tag.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Language(String);

impl Language {
    /// Validates and retains a source language tag.
    ///
    /// # Errors
    ///
    /// Returns [`ModelError::InvalidText`] for an empty or NUL-containing tag.
    pub fn new(value: impl Into<String>) -> Result<Self, ModelError> {
        let value = value.into();
        let trimmed = value.trim();
        validate_text("language", trimmed)?;
        Ok(Self(trimmed.to_owned()))
    }

    /// Returns the retained tag.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Language {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// A positive rational time base.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TimeBase {
    numerator: NonZeroU32,
    denominator: NonZeroU32,
}

impl TimeBase {
    /// Creates a rational time base.
    ///
    /// # Errors
    ///
    /// Returns [`ModelError::InvalidTimeBase`] when either part is zero.
    pub const fn new(numerator: u32, denominator: u32) -> Result<Self, ModelError> {
        match (NonZeroU32::new(numerator), NonZeroU32::new(denominator)) {
            (Some(numerator), Some(denominator)) => Ok(Self {
                numerator,
                denominator,
            }),
            _ => Err(ModelError::InvalidTimeBase),
        }
    }

    /// Returns the numerator.
    #[must_use]
    pub const fn numerator(self) -> u32 {
        self.numerator.get()
    }

    /// Returns the denominator.
    #[must_use]
    pub const fn denominator(self) -> u32 {
        self.denominator.get()
    }
}

/// A timestamp represented exactly as ticks and a rational time base.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MediaTimestamp {
    ticks: i64,
    time_base: TimeBase,
}

impl MediaTimestamp {
    /// Creates a timestamp.
    #[must_use]
    pub const fn new(ticks: i64, time_base: TimeBase) -> Self {
        Self { ticks, time_base }
    }

    /// Returns source ticks.
    #[must_use]
    pub const fn ticks(self) -> i64 {
        self.ticks
    }

    /// Returns the timestamp time base.
    #[must_use]
    pub const fn time_base(self) -> TimeBase {
        self.time_base
    }
}

/// A non-negative duration measured in microseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DurationMicros(u64);

impl DurationMicros {
    /// Creates a duration.
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Returns microseconds.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// Stream timing facts retained from the probe.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StreamTiming {
    start: Option<MediaTimestamp>,
    duration_ticks: Option<i64>,
}

impl StreamTiming {
    /// Creates timing information.
    #[must_use]
    pub const fn new(start: Option<MediaTimestamp>, duration_ticks: Option<i64>) -> Self {
        Self {
            start,
            duration_ticks,
        }
    }

    /// Returns the optional stream start timestamp.
    #[must_use]
    pub const fn start(&self) -> Option<MediaTimestamp> {
        self.start
    }

    /// Returns duration in stream time-base ticks.
    #[must_use]
    pub const fn duration_ticks(&self) -> Option<i64> {
        self.duration_ticks
    }
}

/// Ordered stream or container metadata.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Metadata(BTreeMap<String, String>);

impl Metadata {
    /// Validates an ordered metadata map.
    ///
    /// # Errors
    ///
    /// Returns [`ModelError::InvalidText`] for invalid keys or values.
    pub fn new(values: BTreeMap<String, String>) -> Result<Self, ModelError> {
        for (key, value) in &values {
            validate_text("metadata key", key)?;
            if value.contains('\0') {
                return Err(ModelError::InvalidText {
                    field: "metadata value",
                    reason: "contains a NUL character",
                });
            }
        }
        Ok(Self(values))
    }

    /// Returns a metadata value by its exact key.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).map(String::as_str)
    }

    /// Returns the source language tag when it is valid and present.
    #[must_use]
    pub fn language(&self) -> Option<Language> {
        self.get("language")
            .and_then(|value| Language::new(value).ok())
    }

    /// Returns the source title when present.
    #[must_use]
    pub fn title(&self) -> Option<&str> {
        self.get("title")
    }

    /// Inserts or replaces a metadata value.
    ///
    /// # Errors
    ///
    /// Returns [`ModelError::InvalidText`] for an invalid key or value.
    pub fn insert(
        &mut self,
        key: impl Into<String>,
        value: impl Into<String>,
    ) -> Result<Option<String>, ModelError> {
        let key = key.into();
        let value = value.into();
        validate_text("metadata key", &key)?;
        if value.contains('\0') {
            return Err(ModelError::InvalidText {
                field: "metadata value",
                reason: "contains a NUL character",
            });
        }
        Ok(self.0.insert(key, value))
    }

    /// Iterates over metadata in stable key order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.0
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
    }
}

/// Matroska/FFmpeg stream disposition flags.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Dispositions {
    default: bool,
    dub: bool,
    original: bool,
    comment: bool,
    lyrics: bool,
    karaoke: bool,
    forced: bool,
    hearing_impaired: bool,
    visual_impaired: bool,
    clean_effects: bool,
    attached_pic: bool,
    timed_thumbnails: bool,
    captions: bool,
    descriptions: bool,
    metadata: bool,
    dependent: bool,
    still_image: bool,
    other: BTreeMap<String, bool>,
}

impl Dispositions {
    /// Converts FFprobe disposition flags while retaining unknown names.
    #[must_use]
    pub fn from_flags(mut flags: BTreeMap<String, bool>) -> Self {
        Self {
            default: take_flag(&mut flags, "default"),
            dub: take_flag(&mut flags, "dub"),
            original: take_flag(&mut flags, "original"),
            comment: take_flag(&mut flags, "comment"),
            lyrics: take_flag(&mut flags, "lyrics"),
            karaoke: take_flag(&mut flags, "karaoke"),
            forced: take_flag(&mut flags, "forced"),
            hearing_impaired: take_flag(&mut flags, "hearing_impaired"),
            visual_impaired: take_flag(&mut flags, "visual_impaired"),
            clean_effects: take_flag(&mut flags, "clean_effects"),
            attached_pic: take_flag(&mut flags, "attached_pic"),
            timed_thumbnails: take_flag(&mut flags, "timed_thumbnails"),
            captions: take_flag(&mut flags, "captions"),
            descriptions: take_flag(&mut flags, "descriptions"),
            metadata: take_flag(&mut flags, "metadata"),
            dependent: take_flag(&mut flags, "dependent"),
            still_image: take_flag(&mut flags, "still_image"),
            other: flags,
        }
    }

    /// Returns whether the stream is the default selection.
    #[must_use]
    pub const fn is_default(&self) -> bool {
        self.default
    }

    /// Changes only the default flag.
    pub fn set_default(&mut self, value: bool) {
        self.default = value;
    }

    /// Returns a named known or retained disposition flag.
    #[must_use]
    pub fn flag(&self, name: &str) -> Option<bool> {
        let known = match name {
            "default" => self.default,
            "dub" => self.dub,
            "original" => self.original,
            "comment" => self.comment,
            "lyrics" => self.lyrics,
            "karaoke" => self.karaoke,
            "forced" => self.forced,
            "hearing_impaired" => self.hearing_impaired,
            "visual_impaired" => self.visual_impaired,
            "clean_effects" => self.clean_effects,
            "attached_pic" => self.attached_pic,
            "timed_thumbnails" => self.timed_thumbnails,
            "captions" => self.captions,
            "descriptions" => self.descriptions,
            "metadata" => self.metadata,
            "dependent" => self.dependent,
            "still_image" => self.still_image,
            other => return self.other.get(other).copied(),
        };
        Some(known)
    }

    /// Returns all retained flags in deterministic order.
    #[must_use]
    pub fn to_flags(&self) -> BTreeMap<String, bool> {
        let mut flags = self.other.clone();
        for name in KNOWN_DISPOSITIONS {
            if let Some(value) = self.flag(name) {
                flags.insert((*name).to_owned(), value);
            }
        }
        flags
    }
}

const KNOWN_DISPOSITIONS: &[&str] = &[
    "default",
    "dub",
    "original",
    "comment",
    "lyrics",
    "karaoke",
    "forced",
    "hearing_impaired",
    "visual_impaired",
    "clean_effects",
    "attached_pic",
    "timed_thumbnails",
    "captions",
    "descriptions",
    "metadata",
    "dependent",
    "still_image",
];

/// Facts common to every source stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamCommon {
    index: StreamIndex,
    codec_name: String,
    codec_profile: Option<String>,
    bitrate: Option<Bitrate>,
    timing: StreamTiming,
    metadata: Metadata,
    dispositions: Dispositions,
}

impl StreamCommon {
    /// Creates common stream facts.
    ///
    /// # Errors
    ///
    /// Returns [`ModelError::InvalidText`] for an empty codec name.
    pub fn new(index: StreamIndex, codec_name: impl Into<String>) -> Result<Self, ModelError> {
        let codec_name = codec_name.into();
        validate_text("codec name", &codec_name)?;
        Ok(Self {
            index,
            codec_name,
            codec_profile: None,
            bitrate: None,
            timing: StreamTiming::default(),
            metadata: Metadata::default(),
            dispositions: Dispositions::default(),
        })
    }

    /// Sets the optional codec profile.
    #[must_use]
    pub fn with_profile(mut self, profile: Option<String>) -> Self {
        self.codec_profile = profile;
        self
    }

    /// Sets the optional bitrate.
    #[must_use]
    pub const fn with_bitrate(mut self, bitrate: Option<Bitrate>) -> Self {
        self.bitrate = bitrate;
        self
    }

    /// Sets stream timing.
    #[must_use]
    pub const fn with_timing(mut self, timing: StreamTiming) -> Self {
        self.timing = timing;
        self
    }

    /// Sets stream metadata.
    #[must_use]
    pub fn with_metadata(mut self, metadata: Metadata) -> Self {
        self.metadata = metadata;
        self
    }

    /// Sets stream dispositions.
    #[must_use]
    pub fn with_dispositions(mut self, dispositions: Dispositions) -> Self {
        self.dispositions = dispositions;
        self
    }

    /// Returns the source index.
    #[must_use]
    pub const fn index(&self) -> StreamIndex {
        self.index
    }

    /// Returns FFprobe's codec name.
    #[must_use]
    pub fn codec_name(&self) -> &str {
        &self.codec_name
    }

    /// Returns the optional codec profile.
    #[must_use]
    pub fn codec_profile(&self) -> Option<&str> {
        self.codec_profile.as_deref()
    }

    /// Returns the optional bitrate.
    #[must_use]
    pub const fn bitrate(&self) -> Option<Bitrate> {
        self.bitrate
    }

    /// Returns timing facts.
    #[must_use]
    pub const fn timing(&self) -> &StreamTiming {
        &self.timing
    }

    /// Returns metadata.
    #[must_use]
    pub const fn metadata(&self) -> &Metadata {
        &self.metadata
    }

    /// Returns dispositions.
    #[must_use]
    pub const fn dispositions(&self) -> &Dispositions {
        &self.dispositions
    }
}

/// DTS bitstream profile.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum DtsProfile {
    /// Lossy DTS core.
    Core,
    /// DTS-HD High Resolution Audio.
    HdHighResolution,
    /// DTS-HD Master Audio.
    HdMasterAudio,
    /// DTS Express.
    Express,
    /// A retained profile unknown to this SonicMux version.
    Unknown(String),
}

/// Broad PCM representation.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum PcmKind {
    /// Signed integer PCM.
    Signed,
    /// Unsigned integer PCM.
    Unsigned,
    /// Floating-point PCM.
    Float,
    /// A retained PCM name unknown to this SonicMux version.
    Other(String),
}

/// Validated audio codec identity.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum AudioCodec {
    /// Dolby Digital.
    Ac3,
    /// Dolby Digital Plus.
    Eac3,
    /// Advanced Audio Coding.
    Aac,
    /// MPEG Layer III.
    Mp3,
    /// DTS family with profile.
    Dts(DtsProfile),
    /// Dolby TrueHD.
    TrueHd,
    /// Free Lossless Audio Codec.
    Flac,
    /// Opus.
    Opus,
    /// Vorbis.
    Vorbis,
    /// Pulse-code modulation.
    Pcm(PcmKind),
    /// A retained codec unknown to this SonicMux version.
    Other(String),
}

impl AudioCodec {
    /// Classifies FFprobe codec and profile strings.
    ///
    /// # Examples
    ///
    /// ```
    /// use sonicmux_core::{AudioCodec, DtsProfile};
    ///
    /// assert_eq!(
    ///     AudioCodec::from_ffprobe("dts", Some("DTS-HD MA")),
    ///     AudioCodec::Dts(DtsProfile::HdMasterAudio),
    /// );
    /// ```
    #[must_use]
    pub fn from_ffprobe(codec_name: &str, profile: Option<&str>) -> Self {
        match codec_name.to_ascii_lowercase().as_str() {
            "ac3" => Self::Ac3,
            "eac3" => Self::Eac3,
            "aac" => Self::Aac,
            "mp3" => Self::Mp3,
            "truehd" => Self::TrueHd,
            "flac" => Self::Flac,
            "opus" => Self::Opus,
            "vorbis" => Self::Vorbis,
            "dts" => Self::Dts(classify_dts_profile(profile)),
            name if name.starts_with("pcm_s") => Self::Pcm(PcmKind::Signed),
            name if name.starts_with("pcm_u") => Self::Pcm(PcmKind::Unsigned),
            name if name.starts_with("pcm_f") => Self::Pcm(PcmKind::Float),
            name if name.starts_with("pcm_") => Self::Pcm(PcmKind::Other(name.to_owned())),
            name => Self::Other(name.to_owned()),
        }
    }

    /// Returns the codec family used by compatibility rules.
    #[must_use]
    pub const fn family(&self) -> AudioCodecFamily {
        match self {
            Self::Ac3 => AudioCodecFamily::Ac3,
            Self::Eac3 => AudioCodecFamily::Eac3,
            Self::Aac => AudioCodecFamily::Aac,
            Self::Mp3 => AudioCodecFamily::Mp3,
            Self::Dts(_) => AudioCodecFamily::Dts,
            Self::TrueHd => AudioCodecFamily::TrueHd,
            Self::Flac => AudioCodecFamily::Flac,
            Self::Opus => AudioCodecFamily::Opus,
            Self::Vorbis => AudioCodecFamily::Vorbis,
            Self::Pcm(_) => AudioCodecFamily::Pcm,
            Self::Other(_) => AudioCodecFamily::Other,
        }
    }
}

impl fmt::Display for AudioCodec {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::Ac3 => "AC-3",
            Self::Eac3 => "E-AC-3",
            Self::Aac => "AAC",
            Self::Mp3 => "MP3",
            Self::Dts(DtsProfile::Core) => "DTS",
            Self::Dts(DtsProfile::HdHighResolution) => "DTS-HD HRA",
            Self::Dts(DtsProfile::HdMasterAudio) => "DTS-HD MA",
            Self::Dts(DtsProfile::Express) => "DTS Express",
            Self::Dts(DtsProfile::Unknown(value)) => value,
            Self::TrueHd => "TrueHD",
            Self::Flac => "FLAC",
            Self::Opus => "Opus",
            Self::Vorbis => "Vorbis",
            Self::Pcm(_) => "PCM",
            Self::Other(value) => value,
        };
        formatter.write_str(name)
    }
}

/// Codec family used as an ordered compatibility-policy key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum AudioCodecFamily {
    /// AC-3.
    Ac3,
    /// E-AC-3.
    Eac3,
    /// AAC.
    Aac,
    /// MP3.
    Mp3,
    /// DTS family.
    Dts,
    /// TrueHD.
    TrueHd,
    /// FLAC.
    Flac,
    /// Opus.
    Opus,
    /// Vorbis.
    Vorbis,
    /// PCM family.
    Pcm,
    /// Completely unknown codec.
    Other,
}

/// Channel count and retained FFmpeg layout name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Channels {
    count: ChannelCount,
    layout_name: Option<String>,
}

impl Channels {
    /// Creates audio channel facts.
    #[must_use]
    pub fn new(count: ChannelCount, layout_name: Option<String>) -> Self {
        Self { count, layout_name }
    }

    /// Returns the channel count.
    #[must_use]
    pub const fn count(&self) -> ChannelCount {
        self.count
    }

    /// Returns FFmpeg's optional channel-layout name.
    #[must_use]
    pub fn layout_name(&self) -> Option<&str> {
        self.layout_name.as_deref()
    }
}

/// Audio stream facts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioStream {
    common: StreamCommon,
    codec: AudioCodec,
    channels: Channels,
    sample_rate: Option<SampleRate>,
}

impl AudioStream {
    /// Creates an audio stream.
    #[must_use]
    pub fn new(
        common: StreamCommon,
        codec: AudioCodec,
        channels: Channels,
        sample_rate: Option<SampleRate>,
    ) -> Self {
        Self {
            common,
            codec,
            channels,
            sample_rate,
        }
    }

    /// Returns common stream facts.
    #[must_use]
    pub const fn common(&self) -> &StreamCommon {
        &self.common
    }

    /// Returns the classified codec.
    #[must_use]
    pub const fn codec(&self) -> &AudioCodec {
        &self.codec
    }

    /// Returns channel facts.
    #[must_use]
    pub const fn channels(&self) -> &Channels {
        &self.channels
    }

    /// Returns the optional sample rate.
    #[must_use]
    pub const fn sample_rate(&self) -> Option<SampleRate> {
        self.sample_rate
    }
}

macro_rules! common_stream {
    ($name:ident, $description:literal) => {
        #[doc = $description]
        #[derive(Debug, Clone, PartialEq, Eq)]
        pub struct $name {
            common: StreamCommon,
        }

        impl $name {
            /// Creates the stream from common facts.
            #[must_use]
            pub const fn new(common: StreamCommon) -> Self {
                Self { common }
            }

            /// Returns common stream facts.
            #[must_use]
            pub const fn common(&self) -> &StreamCommon {
                &self.common
            }
        }
    };
}

common_stream!(VideoStream, "Video stream facts.");
common_stream!(SubtitleStream, "Subtitle stream facts.");
common_stream!(DataStream, "Data stream facts.");

/// Attachment stream facts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachmentStream {
    common: StreamCommon,
    filename: Option<String>,
    mime_type: Option<String>,
}

impl AttachmentStream {
    /// Creates an attachment stream.
    #[must_use]
    pub fn new(common: StreamCommon, filename: Option<String>, mime_type: Option<String>) -> Self {
        Self {
            common,
            filename,
            mime_type,
        }
    }

    /// Returns common stream facts.
    #[must_use]
    pub const fn common(&self) -> &StreamCommon {
        &self.common
    }

    /// Returns the optional attachment filename.
    #[must_use]
    pub fn filename(&self) -> Option<&str> {
        self.filename.as_deref()
    }

    /// Returns the optional MIME type.
    #[must_use]
    pub fn mime_type(&self) -> Option<&str> {
        self.mime_type.as_deref()
    }
}

/// A retained stream type unknown to this SonicMux version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownStream {
    common: StreamCommon,
    kind: String,
}

impl UnknownStream {
    /// Creates an unknown stream.
    #[must_use]
    pub fn new(common: StreamCommon, kind: String) -> Self {
        Self { common, kind }
    }

    /// Returns common stream facts.
    #[must_use]
    pub const fn common(&self) -> &StreamCommon {
        &self.common
    }

    /// Returns FFprobe's stream type.
    #[must_use]
    pub fn kind(&self) -> &str {
        &self.kind
    }
}

/// A typed source stream.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum StreamInfo {
    /// Video stream.
    Video(VideoStream),
    /// Audio stream.
    Audio(AudioStream),
    /// Subtitle stream.
    Subtitle(SubtitleStream),
    /// Attachment stream.
    Attachment(AttachmentStream),
    /// Data stream.
    Data(DataStream),
    /// Unknown stream kind retained for safe copying.
    Unknown(UnknownStream),
}

impl StreamInfo {
    /// Returns common stream facts.
    #[must_use]
    pub const fn common(&self) -> &StreamCommon {
        match self {
            Self::Video(stream) => stream.common(),
            Self::Audio(stream) => stream.common(),
            Self::Subtitle(stream) => stream.common(),
            Self::Attachment(stream) => stream.common(),
            Self::Data(stream) => stream.common(),
            Self::Unknown(stream) => stream.common(),
        }
    }

    /// Returns the source stream index.
    #[must_use]
    pub const fn index(&self) -> StreamIndex {
        self.common().index()
    }

    /// Returns the audio stream when this is audio.
    #[must_use]
    pub const fn as_audio(&self) -> Option<&AudioStream> {
        match self {
            Self::Audio(stream) => Some(stream),
            _ => None,
        }
    }
}

/// Container-level facts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormatInfo {
    names: Vec<String>,
    duration: Option<DurationMicros>,
    start_time: Option<i64>,
    bitrate: Option<Bitrate>,
    metadata: Metadata,
}

impl FormatInfo {
    /// Creates format facts from one or more demuxer names.
    ///
    /// # Errors
    ///
    /// Returns [`ModelError::MissingFormatName`] when no usable name exists.
    pub fn new(names: impl IntoIterator<Item = String>) -> Result<Self, ModelError> {
        let names: Vec<String> = names
            .into_iter()
            .map(|name| name.trim().to_ascii_lowercase())
            .filter(|name| !name.is_empty())
            .collect();
        if names.is_empty() {
            return Err(ModelError::MissingFormatName);
        }
        Ok(Self {
            names,
            duration: None,
            start_time: None,
            bitrate: None,
            metadata: Metadata::default(),
        })
    }

    /// Sets optional duration.
    #[must_use]
    pub const fn with_duration(mut self, duration: Option<DurationMicros>) -> Self {
        self.duration = duration;
        self
    }

    /// Sets optional start time in microseconds.
    #[must_use]
    pub const fn with_start_time(mut self, start_time: Option<i64>) -> Self {
        self.start_time = start_time;
        self
    }

    /// Sets aggregate bitrate.
    #[must_use]
    pub const fn with_bitrate(mut self, bitrate: Option<Bitrate>) -> Self {
        self.bitrate = bitrate;
        self
    }

    /// Sets global metadata.
    #[must_use]
    pub fn with_metadata(mut self, metadata: Metadata) -> Self {
        self.metadata = metadata;
        self
    }

    /// Returns detected demuxer names.
    #[must_use]
    pub fn names(&self) -> &[String] {
        &self.names
    }

    /// Returns whether FFprobe identified a Matroska demuxer.
    #[must_use]
    pub fn is_matroska(&self) -> bool {
        self.names.iter().any(|name| name == "matroska")
    }

    /// Returns optional duration.
    #[must_use]
    pub const fn duration(&self) -> Option<DurationMicros> {
        self.duration
    }

    /// Returns optional start time in microseconds.
    #[must_use]
    pub const fn start_time(&self) -> Option<i64> {
        self.start_time
    }

    /// Returns optional aggregate bitrate.
    #[must_use]
    pub const fn bitrate(&self) -> Option<Bitrate> {
        self.bitrate
    }

    /// Returns global metadata.
    #[must_use]
    pub const fn metadata(&self) -> &Metadata {
        &self.metadata
    }
}

/// Chapter boundaries and metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chapter {
    id: i64,
    time_base: TimeBase,
    start: i64,
    end: i64,
    metadata: Metadata,
}

impl Chapter {
    /// Creates a chapter.
    ///
    /// # Errors
    ///
    /// Returns [`ModelError::InvalidChapterRange`] when `end < start`.
    pub fn new(
        id: i64,
        time_base: TimeBase,
        start: i64,
        end: i64,
        metadata: Metadata,
    ) -> Result<Self, ModelError> {
        if end < start {
            return Err(ModelError::InvalidChapterRange { id });
        }
        Ok(Self {
            id,
            time_base,
            start,
            end,
            metadata,
        })
    }

    /// Returns the chapter identifier.
    #[must_use]
    pub const fn id(&self) -> i64 {
        self.id
    }

    /// Returns the chapter time base.
    #[must_use]
    pub const fn time_base(&self) -> TimeBase {
        self.time_base
    }

    /// Returns the start tick.
    #[must_use]
    pub const fn start(&self) -> i64 {
        self.start
    }

    /// Returns the end tick.
    #[must_use]
    pub const fn end(&self) -> i64 {
        self.end
    }

    /// Returns chapter metadata.
    #[must_use]
    pub const fn metadata(&self) -> &Metadata {
        &self.metadata
    }
}

/// Non-fatal probe information retained for reports.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ProbeWarning {
    /// An optional numeric field contained `N/A` or invalid text.
    InvalidOptionalNumber {
        /// Field name.
        field: String,
        /// Stream index when the field belonged to a stream.
        stream: Option<StreamIndex>,
    },
    /// An optional codec name was absent and replaced with `unknown`.
    MissingCodecName {
        /// Affected source stream.
        stream: StreamIndex,
    },
    /// Metadata value could not be represented as a string and was omitted.
    UnsupportedMetadataValue {
        /// Metadata key.
        key: String,
        /// Stream index when applicable.
        stream: Option<StreamIndex>,
    },
}

/// Validated media probe result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaInfo {
    path: PathBuf,
    format: FormatInfo,
    streams: Vec<StreamInfo>,
    chapters: Vec<Chapter>,
    warnings: Vec<ProbeWarning>,
}

impl MediaInfo {
    /// Creates validated media information.
    ///
    /// # Errors
    ///
    /// Returns [`ModelError`] for an empty stream list or duplicate indices.
    pub fn new(
        path: PathBuf,
        format: FormatInfo,
        streams: Vec<StreamInfo>,
        chapters: Vec<Chapter>,
    ) -> Result<Self, ModelError> {
        if streams.is_empty() {
            return Err(ModelError::NoStreams);
        }
        let mut indices = BTreeSet::new();
        for stream in &streams {
            if !indices.insert(stream.index()) {
                return Err(ModelError::DuplicateStreamIndex {
                    index: stream.index(),
                });
            }
        }
        Ok(Self {
            path,
            format,
            streams,
            chapters,
            warnings: Vec::new(),
        })
    }

    /// Sets non-fatal probe warnings.
    #[must_use]
    pub fn with_warnings(mut self, warnings: Vec<ProbeWarning>) -> Self {
        self.warnings = warnings;
        self
    }

    /// Returns the input path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Returns container facts.
    #[must_use]
    pub const fn format(&self) -> &FormatInfo {
        &self.format
    }

    /// Returns source streams in demuxer order.
    #[must_use]
    pub fn streams(&self) -> &[StreamInfo] {
        &self.streams
    }

    /// Iterates over audio streams in source order.
    pub fn audio_streams(&self) -> impl Iterator<Item = &AudioStream> {
        self.streams.iter().filter_map(StreamInfo::as_audio)
    }

    /// Returns chapters.
    #[must_use]
    pub fn chapters(&self) -> &[Chapter] {
        &self.chapters
    }

    /// Returns non-fatal probe warnings.
    #[must_use]
    pub fn warnings(&self) -> &[ProbeWarning] {
        &self.warnings
    }
}

fn validate_text(field: &'static str, value: &str) -> Result<(), ModelError> {
    if value.trim().is_empty() {
        return Err(ModelError::InvalidText {
            field,
            reason: "is empty",
        });
    }
    if value.contains('\0') {
        return Err(ModelError::InvalidText {
            field,
            reason: "contains a NUL character",
        });
    }
    Ok(())
}

fn take_flag(flags: &mut BTreeMap<String, bool>, name: &str) -> bool {
    flags.remove(name).unwrap_or(false)
}

fn classify_dts_profile(profile: Option<&str>) -> DtsProfile {
    let Some(profile) = profile else {
        return DtsProfile::Core;
    };
    let normalized = profile.to_ascii_lowercase();
    if normalized.contains("master audio") || normalized.contains("dts-hd ma") {
        DtsProfile::HdMasterAudio
    } else if normalized.contains("high resolution") || normalized.contains("dts-hd hra") {
        DtsProfile::HdHighResolution
    } else if normalized.contains("express") {
        DtsProfile::Express
    } else if normalized == "dts" || normalized.contains("core") {
        DtsProfile::Core
    } else {
        DtsProfile::Unknown(profile.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::{AudioCodec, Bitrate, DtsProfile, Language, TimeBase};

    #[test]
    fn bitrate_supports_values_above_u32() {
        let bitrate = Bitrate::new(u64::from(u32::MAX) + 1);
        assert_eq!(bitrate.map(Bitrate::get), Ok(u64::from(u32::MAX) + 1));
    }

    #[test]
    fn language_retains_non_bcp47_source_tag() {
        let language = Language::new(" qaa ");
        assert_eq!(language.as_ref().map(Language::as_str), Ok("qaa"));
    }

    #[test]
    fn time_base_rejects_zero_denominator() {
        assert!(TimeBase::new(1, 0).is_err());
    }

    #[test]
    fn classifies_dts_hd_master_audio_profile() {
        assert_eq!(
            AudioCodec::from_ffprobe("dts", Some("DTS-HD MA")),
            AudioCodec::Dts(DtsProfile::HdMasterAudio)
        );
    }

    #[test]
    fn classifies_unknown_dts_profile_without_losing_it() {
        assert_eq!(
            AudioCodec::from_ffprobe("dts", Some("future profile")),
            AudioCodec::Dts(DtsProfile::Unknown("future profile".to_owned()))
        );
    }
}
