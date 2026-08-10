//! Device compatibility policies for audio streams.

use std::collections::{BTreeMap, BTreeSet};

use thiserror::Error;

use crate::{AudioCodec, AudioCodecFamily, AudioStream, ChannelCount};

/// Name of a built-in or custom device profile.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum ProfileName {
    /// Conservative generic television baseline.
    GenericTv,
    /// Conservative Samsung baseline pending model-specific sourced rules.
    Samsung,
    /// Conservative LG baseline pending model-specific sourced rules.
    Lg,
    /// Conservative DLNA baseline pending renderer-specific sourced rules.
    Dlna,
    /// User-defined profile.
    Custom(String),
}

/// Behavior when an audio codec is completely unknown to SonicMux.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum UnknownCodecBehavior {
    /// Reject planning because decodability cannot be established.
    Reject,
    /// Allow the execution backend to attempt decoding into the configured target.
    TranscodeWithFallback,
}

/// Compatibility constraints for one codec family.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodecRule {
    maximum_channels: Option<ChannelCount>,
    allowed_layouts: Option<BTreeSet<String>>,
}

impl CodecRule {
    /// Creates a codec rule.
    #[must_use]
    pub fn new(
        maximum_channels: Option<ChannelCount>,
        allowed_layouts: Option<BTreeSet<String>>,
    ) -> Self {
        Self {
            maximum_channels,
            allowed_layouts,
        }
    }

    /// Returns the optional maximum channel count.
    #[must_use]
    pub const fn maximum_channels(&self) -> Option<ChannelCount> {
        self.maximum_channels
    }

    /// Returns the optional layout allow-list.
    #[must_use]
    pub const fn allowed_layouts(&self) -> Option<&BTreeSet<String>> {
        self.allowed_layouts.as_ref()
    }
}

/// A typed reason why an audio stream is incompatible.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
#[non_exhaustive]
pub enum IncompatibilityReason {
    /// The codec family is not supported by the profile.
    UnsupportedCodec(AudioCodecFamily),
    /// The stream exceeds the profile's channel limit.
    TooManyChannels {
        /// Actual source channels.
        actual: ChannelCount,
        /// Maximum supported channels.
        maximum: ChannelCount,
    },
    /// A reported layout is outside the profile's allow-list.
    UnsupportedLayout(String),
}

/// One or more incompatibility reasons.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Incompatibilities {
    primary: IncompatibilityReason,
    additional: Vec<IncompatibilityReason>,
}

impl Incompatibilities {
    fn from_reasons(mut reasons: Vec<IncompatibilityReason>) -> Option<Self> {
        if reasons.is_empty() {
            return None;
        }
        reasons.sort();
        let primary = reasons.remove(0);
        Some(Self {
            primary,
            additional: reasons,
        })
    }

    /// Returns the first stable reason.
    #[must_use]
    pub const fn primary(&self) -> &IncompatibilityReason {
        &self.primary
    }

    /// Iterates over every reason in stable order.
    pub fn iter(&self) -> impl Iterator<Item = &IncompatibilityReason> {
        std::iter::once(&self.primary).chain(self.additional.iter())
    }
}

/// Compatibility classification for an audio stream.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Compatibility {
    /// The active profile accepts the stream without transcoding.
    Compatible,
    /// The stream requires a derivative.
    Incompatible(Incompatibilities),
}

impl Compatibility {
    /// Returns whether the stream can be copied as compatible audio.
    #[must_use]
    pub const fn is_compatible(&self) -> bool {
        matches!(self, Self::Compatible)
    }
}

/// Compatibility-policy evaluation error.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum PolicyError {
    /// The codec is unknown and the profile does not allow fallback decoding.
    #[error("unknown audio codec `{codec}` is not allowed by this profile")]
    UnknownCodec {
        /// Retained FFprobe codec name.
        codec: String,
    },
}

/// Ordered rules used to classify source audio.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompatibilityPolicy {
    name: ProfileName,
    description: String,
    conservative_baseline: bool,
    rules: BTreeMap<AudioCodecFamily, CodecRule>,
    unknown_codec: UnknownCodecBehavior,
}

impl CompatibilityPolicy {
    /// Creates one of the built-in conservative policies.
    #[must_use]
    pub fn for_profile(name: ProfileName) -> Self {
        let maximum_surround = ChannelCount::new(6).ok();
        let maximum_stereo = ChannelCount::new(2).ok();
        let mut rules = BTreeMap::new();
        rules.insert(
            AudioCodecFamily::Ac3,
            CodecRule::new(maximum_surround, None),
        );
        rules.insert(AudioCodecFamily::Aac, CodecRule::new(maximum_stereo, None));
        rules.insert(AudioCodecFamily::Mp3, CodecRule::new(maximum_stereo, None));
        let (description, conservative_baseline) = match &name {
            ProfileName::GenericTv => ("conservative generic television baseline", false),
            ProfileName::Samsung => ("conservative Samsung baseline; model support varies", true),
            ProfileName::Lg => ("conservative LG baseline; model support varies", true),
            ProfileName::Dlna => ("conservative DLNA baseline; renderer support varies", true),
            ProfileName::Custom(value) => (value.as_str(), false),
        };
        let description = description.to_owned();
        Self {
            name,
            description,
            conservative_baseline,
            rules,
            unknown_codec: UnknownCodecBehavior::Reject,
        }
    }

    /// Replaces or adds a codec rule.
    #[must_use]
    pub fn with_rule(mut self, family: AudioCodecFamily, rule: CodecRule) -> Self {
        self.rules.insert(family, rule);
        self
    }

    /// Sets unknown-codec behavior.
    #[must_use]
    pub const fn with_unknown_codec(mut self, behavior: UnknownCodecBehavior) -> Self {
        self.unknown_codec = behavior;
        self
    }

    /// Returns the profile name.
    #[must_use]
    pub const fn name(&self) -> &ProfileName {
        &self.name
    }

    /// Returns the profile description.
    #[must_use]
    pub fn description(&self) -> &str {
        &self.description
    }

    /// Returns whether this is a deliberately conservative vendor/protocol baseline.
    #[must_use]
    pub const fn is_conservative_baseline(&self) -> bool {
        self.conservative_baseline
    }

    /// Returns unknown-codec behavior.
    #[must_use]
    pub const fn unknown_codec_behavior(&self) -> UnknownCodecBehavior {
        self.unknown_codec
    }

    /// Classifies one audio stream.
    ///
    /// # Errors
    ///
    /// Returns [`PolicyError::UnknownCodec`] when the codec is completely unknown
    /// and fallback decoding was not explicitly enabled.
    pub fn classify(&self, stream: &AudioStream) -> Result<Compatibility, PolicyError> {
        if let AudioCodec::Other(codec) = stream.codec() {
            if self.unknown_codec == UnknownCodecBehavior::Reject {
                return Err(PolicyError::UnknownCodec {
                    codec: codec.clone(),
                });
            }
        }

        let family = stream.codec().family();
        let Some(rule) = self.rules.get(&family) else {
            return Ok(Compatibility::Incompatible(Incompatibilities {
                primary: IncompatibilityReason::UnsupportedCodec(family),
                additional: Vec::new(),
            }));
        };

        let mut reasons = Vec::new();
        if let Some(maximum) = rule.maximum_channels {
            let actual = stream.channels().count();
            if actual > maximum {
                reasons.push(IncompatibilityReason::TooManyChannels { actual, maximum });
            }
        }
        if let (Some(allowed), Some(layout)) =
            (&rule.allowed_layouts, stream.channels().layout_name())
        {
            if !allowed.contains(layout) {
                reasons.push(IncompatibilityReason::UnsupportedLayout(layout.to_owned()));
            }
        }

        Ok(match Incompatibilities::from_reasons(reasons) {
            Some(reasons) => Compatibility::Incompatible(reasons),
            None => Compatibility::Compatible,
        })
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests {
    use std::collections::BTreeSet;

    use proptest::prelude::*;

    use super::{
        CodecRule, Compatibility, CompatibilityPolicy, IncompatibilityReason, PolicyError,
        ProfileName, UnknownCodecBehavior,
    };
    use crate::{
        AudioCodec, AudioCodecFamily, AudioStream, ChannelCount, Channels, DtsProfile,
        StreamCommon, StreamIndex,
    };

    fn audio(codec: AudioCodec, channels: u16, layout: Option<&str>) -> AudioStream {
        let common =
            StreamCommon::new(StreamIndex::new(1), "test-codec").expect("test codec name is valid");
        AudioStream::new(
            common,
            codec,
            Channels::new(
                ChannelCount::new(channels).expect("test channels are positive"),
                layout.map(str::to_owned),
            ),
            None,
        )
    }

    fn generic() -> CompatibilityPolicy {
        CompatibilityPolicy::for_profile(ProfileName::GenericTv)
    }

    #[test]
    fn generic_ac3_is_compatible() {
        assert_eq!(
            generic().classify(&audio(AudioCodec::Ac3, 6, Some("5.1"))),
            Ok(Compatibility::Compatible)
        );
    }

    #[test]
    fn generic_stereo_aac_is_compatible() {
        assert_eq!(
            generic().classify(&audio(AudioCodec::Aac, 2, Some("stereo"))),
            Ok(Compatibility::Compatible)
        );
    }

    #[test]
    fn generic_mp3_is_compatible() {
        assert_eq!(
            generic().classify(&audio(AudioCodec::Mp3, 2, Some("stereo"))),
            Ok(Compatibility::Compatible)
        );
    }

    #[test]
    fn dts_core_is_incompatible() {
        let result = generic().classify(&audio(AudioCodec::Dts(DtsProfile::Core), 6, None));
        assert!(matches!(result, Ok(Compatibility::Incompatible(_))));
    }

    #[test]
    fn dts_hd_ma_is_incompatible() {
        let result =
            generic().classify(&audio(AudioCodec::Dts(DtsProfile::HdMasterAudio), 8, None));
        assert!(matches!(result, Ok(Compatibility::Incompatible(_))));
    }

    #[test]
    fn unknown_dts_profile_stays_incompatible() {
        let result = generic().classify(&audio(
            AudioCodec::Dts(DtsProfile::Unknown("future".to_owned())),
            6,
            None,
        ));
        assert!(matches!(result, Ok(Compatibility::Incompatible(_))));
    }

    #[test]
    fn truehd_is_incompatible() {
        let result = generic().classify(&audio(AudioCodec::TrueHd, 8, None));
        assert!(matches!(result, Ok(Compatibility::Incompatible(_))));
    }

    #[test]
    fn known_flac_can_target_fallback_conversion() {
        let result = generic().classify(&audio(AudioCodec::Flac, 6, None));
        assert!(matches!(result, Ok(Compatibility::Incompatible(_))));
    }

    #[test]
    fn completely_unknown_codec_is_rejected() {
        assert_eq!(
            generic().classify(&audio(AudioCodec::Other("future".to_owned()), 2, None)),
            Err(PolicyError::UnknownCodec {
                codec: "future".to_owned()
            })
        );
    }

    #[test]
    fn custom_policy_allows_unknown_fallback() {
        let policy = generic().with_unknown_codec(UnknownCodecBehavior::TranscodeWithFallback);
        let result = policy.classify(&audio(AudioCodec::Other("future".to_owned()), 2, None));
        assert!(matches!(result, Ok(Compatibility::Incompatible(_))));
    }

    #[test]
    fn maximum_channels_reports_typed_reason() {
        let result = generic()
            .classify(&audio(AudioCodec::Aac, 6, Some("5.1")))
            .expect("known codec classification succeeds");
        let Compatibility::Incompatible(reasons) = result else {
            panic!("multichannel AAC must be incompatible");
        };
        assert!(matches!(
            reasons.primary(),
            IncompatibilityReason::TooManyChannels { .. }
        ));
    }

    #[test]
    fn incompatibility_reason_order_is_stable() {
        let allowed = BTreeSet::from(["stereo".to_owned()]);
        let policy = generic().with_rule(
            AudioCodecFamily::Aac,
            CodecRule::new(ChannelCount::new(2).ok(), Some(allowed)),
        );
        let result = policy
            .classify(&audio(AudioCodec::Aac, 6, Some("5.1")))
            .expect("known codec classification succeeds");
        let Compatibility::Incompatible(reasons) = result else {
            panic!("stream must be incompatible");
        };
        assert_eq!(reasons.iter().count(), 2);
        assert!(matches!(
            reasons.primary(),
            IncompatibilityReason::TooManyChannels { .. }
        ));
    }

    #[test]
    fn custom_codec_override_replaces_inherited_rule() {
        let policy = generic().with_rule(
            AudioCodecFamily::Aac,
            CodecRule::new(ChannelCount::new(8).ok(), None),
        );
        assert_eq!(
            policy.classify(&audio(AudioCodec::Aac, 6, Some("5.1"))),
            Ok(Compatibility::Compatible)
        );
    }

    #[test]
    fn vendor_profile_identifies_conservative_baseline() {
        let policy = CompatibilityPolicy::for_profile(ProfileName::Samsung);
        assert!(policy.is_conservative_baseline());
        assert!(policy.description().contains("model support varies"));
    }

    proptest! {
        #[test]
        fn arbitrary_unknown_codec_names_never_panic(codec in ".{0,128}") {
            let result = generic().classify(&audio(AudioCodec::Other(codec.clone()), 2, None));
            prop_assert_eq!(result, Err(PolicyError::UnknownCodec { codec }));
        }

        #[test]
        fn arbitrary_unknown_dts_profiles_remain_incompatible(profile in ".{0,128}") {
            let result = generic().classify(&audio(AudioCodec::Dts(DtsProfile::Unknown(profile)), 6, None));
            prop_assert!(matches!(result, Ok(Compatibility::Incompatible(_))));
        }
    }
}
