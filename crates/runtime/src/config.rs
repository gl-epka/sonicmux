//! Strict versioned configuration loading and precedence merging.

use std::{collections::BTreeMap, env, fs, io, path::PathBuf};

use directories::ProjectDirs;
use serde::Deserialize;
use sonicmux_core::{
    AacBitrate, Ac3Bitrate, AudioCodecFamily, AudioTarget, ChannelCount, CodecRule,
    CompatibilityPolicy, Eac3Bitrate, OutputMode, ProfileName, TargetLayout, UnknownCodecBehavior,
};
use thiserror::Error;

const MAX_CONFIG_BYTES: u64 = 1024 * 1024;
const BUILTIN_PROFILES: &[&str] = &["generic-tv", "samsung", "lg", "dlna"];

/// Origin of one effective configuration value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigSource {
    /// Command-line override.
    Cli,
    /// `SONICMUX_*` environment variable.
    Environment,
    /// Selected TOML file.
    File,
    /// Built-in default.
    Default,
}

impl std::fmt::Display for ConfigSource {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Cli => "cli",
            Self::Environment => "env",
            Self::File => "config",
            Self::Default => "default",
        })
    }
}

/// Effective value paired with its winning source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sourced<T> {
    value: T,
    source: ConfigSource,
}

impl<T> Sourced<T> {
    fn new(value: T, source: ConfigSource) -> Self {
        Self { value, source }
    }

    /// Returns the value.
    #[must_use]
    pub const fn value(&self) -> &T {
        &self.value
    }

    /// Returns its source.
    #[must_use]
    pub const fn source(&self) -> ConfigSource {
        self.source
    }
}

/// Typed partial values from one precedence level.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PartialConfig {
    /// Device profile name.
    pub profile: Option<String>,
    /// Target codec name.
    pub codec: Option<String>,
    /// Target bitrate spelling.
    pub bitrate: Option<String>,
    /// Target channel-layout spelling.
    pub channels: Option<String>,
    /// Audio output-mode spelling.
    pub mode: Option<String>,
    /// FFmpeg executable or installation directory.
    pub ffmpeg_path: Option<PathBuf>,
    /// Output directory.
    pub output_directory: Option<PathBuf>,
    /// Color mode.
    pub color: Option<String>,
    /// Structured diagnostic log file.
    pub log_file: Option<PathBuf>,
    custom_profiles: BTreeMap<String, ProfileFile>,
}

/// Built-in defaults before precedence is applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefaultConfig {
    /// Default device profile.
    pub profile: String,
    /// Default target codec.
    pub codec: String,
    /// Default target bitrate.
    pub bitrate: String,
    /// Default target channels.
    pub channels: String,
    /// Default output mode.
    pub mode: String,
    /// Default color behavior.
    pub color: String,
}

impl Default for DefaultConfig {
    fn default() -> Self {
        Self {
            profile: "generic-tv".to_owned(),
            codec: "ac3".to_owned(),
            bitrate: "640k".to_owned(),
            channels: "keep-up-to-5.1".to_owned(),
            mode: "add".to_owned(),
            color: "auto".to_owned(),
        }
    }
}

/// Fully merged and validated configuration.
#[derive(Debug, Clone)]
pub struct EffectiveConfig {
    /// Device profile.
    pub profile: Sourced<String>,
    /// Target codec.
    pub codec: Sourced<String>,
    /// Target bitrate.
    pub bitrate: Sourced<String>,
    /// Target channels.
    pub channels: Sourced<String>,
    /// Output mode.
    pub mode: Sourced<String>,
    /// Optional FFmpeg path.
    pub ffmpeg_path: Option<Sourced<PathBuf>>,
    /// Optional output directory.
    pub output_directory: Option<Sourced<PathBuf>>,
    /// Color behavior.
    pub color: Sourced<String>,
    /// Optional structured log file.
    pub log_file: Option<Sourced<PathBuf>>,
    custom_profiles: BTreeMap<String, ProfileFile>,
}

impl EffectiveConfig {
    /// Builds the selected compatibility policy.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] for an unknown or invalid custom profile.
    pub fn compatibility_policy(&self) -> Result<CompatibilityPolicy, ConfigError> {
        self.compatibility_policy_named(self.profile.value.as_str())
    }

    /// Builds one named built-in or configured compatibility policy.
    pub fn compatibility_policy_named(
        &self,
        name: &str,
    ) -> Result<CompatibilityPolicy, ConfigError> {
        let profile_name = match name {
            "generic-tv" => ProfileName::GenericTv,
            "samsung" => ProfileName::Samsung,
            "lg" => ProfileName::Lg,
            "dlna" => ProfileName::Dlna,
            custom if self.custom_profiles.contains_key(custom) => {
                ProfileName::Custom(custom.to_owned())
            }
            _ => {
                return Err(ConfigError::UnknownProfile {
                    name: name.to_owned(),
                });
            }
        };
        let mut policy = CompatibilityPolicy::for_profile(profile_name);
        if let Some(custom) = self.custom_profiles.get(name) {
            policy = policy.with_unknown_codec(match custom.unknown_codec {
                UnknownCodecFile::Reject => UnknownCodecBehavior::Reject,
                UnknownCodecFile::TranscodeWithFallback => {
                    UnknownCodecBehavior::TranscodeWithFallback
                }
            });
            for (family, rule) in &custom.codecs {
                policy = policy.with_rule(
                    parse_family(family)?,
                    CodecRule::new(
                        rule.maximum_channels
                            .map(ChannelCount::new)
                            .transpose()
                            .map_err(|error| ConfigError::InvalidValue {
                                field: "maximum-channels",
                                reason: error.to_string(),
                            })?,
                        rule.allowed_layouts
                            .clone()
                            .map(IntoIterator::into_iter)
                            .map(collect_set),
                    ),
                );
            }
        }
        Ok(policy)
    }

    /// Builds the validated target audio settings.
    pub fn audio_target(&self) -> Result<AudioTarget, ConfigError> {
        let bitrate = parse_bitrate(&self.bitrate.value)?;
        let layout = parse_layout(&self.channels.value)?;
        match self.codec.value.as_str() {
            "ac3" => Ac3Bitrate::new(bitrate)
                .map(|bitrate| AudioTarget::Ac3 { bitrate, layout })
                .map_err(invalid_bitrate),
            "eac3" => Eac3Bitrate::new(bitrate)
                .map(|bitrate| AudioTarget::Eac3 { bitrate, layout })
                .map_err(invalid_bitrate),
            "aac" => AacBitrate::new(bitrate)
                .map(|bitrate| AudioTarget::Aac { bitrate, layout })
                .map_err(invalid_bitrate),
            value => Err(ConfigError::InvalidValue {
                field: "codec",
                reason: format!("unsupported codec `{value}`"),
            }),
        }
    }

    /// Returns the validated output mode.
    pub fn output_mode(&self) -> Result<OutputMode, ConfigError> {
        match self.mode.value.as_str() {
            "add" => Ok(OutputMode::Add),
            "replace" => Ok(OutputMode::Replace),
            "only-new" => Ok(OutputMode::OnlyNew),
            value => Err(ConfigError::InvalidValue {
                field: "mode",
                reason: format!("unsupported mode `{value}`"),
            }),
        }
    }

    /// Iterates built-in and configured profile names.
    pub fn profile_names(&self) -> impl Iterator<Item = &str> {
        BUILTIN_PROFILES
            .iter()
            .copied()
            .chain(self.custom_profiles.keys().map(String::as_str))
    }
}

/// Selected configuration path and whether absence is an error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigPath {
    /// Path to read or create.
    pub path: PathBuf,
    /// Whether the user selected it explicitly.
    pub required: bool,
}

/// Configuration loading or validation failure.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ConfigError {
    /// Platform configuration directory is unavailable.
    #[error("could not determine the platform configuration directory")]
    NoPlatformDirectory,
    /// File metadata or content could not be read.
    #[error("failed to read configuration {}: {source}", path.display())]
    Read {
        /// Selected path.
        path: PathBuf,
        /// Operating-system error.
        #[source]
        source: io::Error,
    },
    /// The selected path is not a regular file.
    #[error("configuration is not a regular file: {}", path.display())]
    NotRegular {
        /// Selected path.
        path: PathBuf,
    },
    /// Configuration exceeds the bounded input limit.
    #[error("configuration exceeds the 1 MiB limit: {}", path.display())]
    TooLarge {
        /// Selected path.
        path: PathBuf,
    },
    /// TOML syntax or shape is invalid.
    #[error("invalid configuration TOML: {reason}")]
    Toml {
        /// Bounded parser diagnostic.
        reason: String,
    },
    /// Schema version is not supported.
    #[error("unsupported configuration version {version}")]
    UnsupportedVersion {
        /// Rejected version.
        version: u32,
    },
    /// One typed value is invalid.
    #[error("invalid configuration field `{field}`: {reason}")]
    InvalidValue {
        /// Stable field name.
        field: &'static str,
        /// Explanation.
        reason: String,
    },
    /// Selected profile does not exist.
    #[error("unknown profile `{name}`")]
    UnknownProfile {
        /// Requested profile.
        name: String,
    },
    /// A configured profile shadows a built-in name.
    #[error("configured profile `{name}` collides with a built-in profile")]
    ProfileCollision {
        /// Rejected name.
        name: String,
    },
    /// Environment text was not Unicode.
    #[error("environment variable {name} must contain Unicode text")]
    InvalidEnvironment {
        /// Variable name.
        name: &'static str,
    },
    /// Configuration creation failed.
    #[error("failed to create configuration {}: {source}", path.display())]
    Create {
        /// Destination.
        path: PathBuf,
        /// Operating-system error.
        #[source]
        source: io::Error,
    },
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
struct FileConfig {
    version: Option<u32>,
    profile: Option<String>,
    audio: AudioFile,
    ffmpeg: FfmpegFile,
    output: OutputFile,
    profiles: BTreeMap<String, ProfileFile>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
struct AudioFile {
    codec: Option<String>,
    bitrate: Option<String>,
    channels: Option<String>,
    mode: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
struct FfmpegFile {
    path: Option<PathBuf>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
struct OutputFile {
    directory: Option<PathBuf>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields, rename_all = "kebab-case")]
struct ProfileFile {
    unknown_codec: UnknownCodecFile,
    codecs: BTreeMap<String, CodecRuleFile>,
}

impl Default for ProfileFile {
    fn default() -> Self {
        Self {
            unknown_codec: UnknownCodecFile::Reject,
            codecs: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
enum UnknownCodecFile {
    #[default]
    Reject,
    TranscodeWithFallback,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields, rename_all = "kebab-case")]
struct CodecRuleFile {
    maximum_channels: Option<u16>,
    allowed_layouts: Option<Vec<String>>,
}

/// Resolves CLI, environment, or platform-default configuration location.
pub fn select_config_path(cli: Option<PathBuf>) -> Result<ConfigPath, ConfigError> {
    if let Some(path) = cli {
        return Ok(ConfigPath {
            path,
            required: true,
        });
    }
    if let Some(path) = env::var_os("SONICMUX_CONFIG") {
        return Ok(ConfigPath {
            path: PathBuf::from(path),
            required: true,
        });
    }
    let directories =
        ProjectDirs::from("", "", "sonicmux").ok_or(ConfigError::NoPlatformDirectory)?;
    Ok(ConfigPath {
        path: directories.config_dir().join("config.toml"),
        required: false,
    })
}

/// Reads one strict versioned TOML source.
pub fn load_file(path: &ConfigPath) -> Result<PartialConfig, ConfigError> {
    let metadata = match fs::symlink_metadata(&path.path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound && !path.required => {
            return Ok(PartialConfig::default());
        }
        Err(source) => {
            return Err(ConfigError::Read {
                path: path.path.clone(),
                source,
            });
        }
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(ConfigError::NotRegular {
            path: path.path.clone(),
        });
    }
    if metadata.len() > MAX_CONFIG_BYTES {
        return Err(ConfigError::TooLarge {
            path: path.path.clone(),
        });
    }
    let text = fs::read_to_string(&path.path).map_err(|source| ConfigError::Read {
        path: path.path.clone(),
        source,
    })?;
    let parsed: FileConfig = toml::from_str(&text).map_err(|error| ConfigError::Toml {
        reason: error.to_string(),
    })?;
    if parsed.version.unwrap_or(1) != 1 {
        return Err(ConfigError::UnsupportedVersion {
            version: parsed.version.unwrap_or_default(),
        });
    }
    for name in parsed.profiles.keys() {
        if name.trim().is_empty() {
            return Err(ConfigError::InvalidValue {
                field: "profile name",
                reason: "profile name must not be empty".to_owned(),
            });
        }
        if BUILTIN_PROFILES.contains(&name.as_str()) {
            return Err(ConfigError::ProfileCollision { name: name.clone() });
        }
    }
    validate_profiles(&parsed.profiles)?;
    Ok(PartialConfig {
        profile: parsed.profile,
        codec: parsed.audio.codec,
        bitrate: parsed.audio.bitrate,
        channels: parsed.audio.channels,
        mode: parsed.audio.mode,
        ffmpeg_path: parsed.ffmpeg.path,
        output_directory: parsed.output.directory,
        color: None,
        log_file: None,
        custom_profiles: parsed.profiles,
    })
}

/// Parses the finite supported `SONICMUX_*` environment surface.
pub fn environment_config() -> Result<PartialConfig, ConfigError> {
    Ok(PartialConfig {
        profile: env_text("SONICMUX_PROFILE")?,
        codec: env_text("SONICMUX_CODEC")?,
        bitrate: env_text("SONICMUX_BITRATE")?,
        channels: env_text("SONICMUX_CHANNELS")?,
        mode: env_text("SONICMUX_MODE")?,
        ffmpeg_path: env::var_os("SONICMUX_FFMPEG_PATH").map(PathBuf::from),
        output_directory: env::var_os("SONICMUX_OUTPUT_DIR").map(PathBuf::from),
        color: env_text("SONICMUX_COLOR")?,
        log_file: env::var_os("SONICMUX_LOG_FILE").map(PathBuf::from),
        custom_profiles: BTreeMap::new(),
    })
}

/// Applies CLI > environment > file > defaults and validates the result.
pub fn merge_config(
    defaults: DefaultConfig,
    file: PartialConfig,
    environment: PartialConfig,
    cli: PartialConfig,
) -> Result<EffectiveConfig, ConfigError> {
    let custom_profiles = file.custom_profiles.clone();
    let effective = EffectiveConfig {
        profile: choose(
            cli.profile,
            environment.profile,
            file.profile,
            defaults.profile,
        ),
        codec: choose(cli.codec, environment.codec, file.codec, defaults.codec),
        bitrate: choose(
            cli.bitrate,
            environment.bitrate,
            file.bitrate,
            defaults.bitrate,
        ),
        channels: choose(
            cli.channels,
            environment.channels,
            file.channels,
            defaults.channels,
        ),
        mode: choose(cli.mode, environment.mode, file.mode, defaults.mode),
        ffmpeg_path: choose_optional(cli.ffmpeg_path, environment.ffmpeg_path, file.ffmpeg_path),
        output_directory: choose_optional(
            cli.output_directory,
            environment.output_directory,
            file.output_directory,
        ),
        color: choose(cli.color, environment.color, file.color, defaults.color),
        log_file: choose_optional(cli.log_file, environment.log_file, file.log_file),
        custom_profiles,
    };
    let _policy = effective.compatibility_policy()?;
    let _target = effective.audio_target()?;
    let _mode = effective.output_mode()?;
    if !matches!(effective.color.value.as_str(), "auto" | "always" | "never") {
        return Err(ConfigError::InvalidValue {
            field: "color",
            reason: "expected auto, always, or never".to_owned(),
        });
    }
    Ok(effective)
}

/// Loads and merges all configuration sources.
pub fn load_effective_config(
    path: &ConfigPath,
    cli: PartialConfig,
) -> Result<EffectiveConfig, ConfigError> {
    merge_config(
        DefaultConfig::default(),
        load_file(path)?,
        environment_config()?,
        cli,
    )
}

/// Creates a documented starter file with create-new semantics.
pub fn initialize_config(path: &ConfigPath) -> Result<(), ConfigError> {
    if let Some(parent) = path.path.parent() {
        fs::create_dir_all(parent).map_err(|source| ConfigError::Create {
            path: path.path.clone(),
            source,
        })?;
    }
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path.path)
        .map_err(|source| ConfigError::Create {
            path: path.path.clone(),
            source,
        })?;
    use std::io::Write as _;
    file.write_all(STARTER_CONFIG.as_bytes())
        .and_then(|()| file.sync_all())
        .map_err(|source| ConfigError::Create {
            path: path.path.clone(),
            source,
        })
}

/// Starter configuration written by `config init`.
pub const STARTER_CONFIG: &str = "version = 1\nprofile = \"generic-tv\"\n\n[audio]\ncodec = \"ac3\"\nbitrate = \"640k\"\nchannels = \"keep-up-to-5.1\"\nmode = \"add\"\n";

fn env_text(name: &'static str) -> Result<Option<String>, ConfigError> {
    match env::var(name) {
        Ok(value) => Ok(Some(value)),
        Err(env::VarError::NotPresent) => Ok(None),
        Err(env::VarError::NotUnicode(_)) => Err(ConfigError::InvalidEnvironment { name }),
    }
}

fn choose<T>(cli: Option<T>, env: Option<T>, file: Option<T>, default: T) -> Sourced<T> {
    cli.map(|value| Sourced::new(value, ConfigSource::Cli))
        .or_else(|| env.map(|value| Sourced::new(value, ConfigSource::Environment)))
        .or_else(|| file.map(|value| Sourced::new(value, ConfigSource::File)))
        .unwrap_or_else(|| Sourced::new(default, ConfigSource::Default))
}

fn choose_optional<T>(cli: Option<T>, env: Option<T>, file: Option<T>) -> Option<Sourced<T>> {
    cli.map(|value| Sourced::new(value, ConfigSource::Cli))
        .or_else(|| env.map(|value| Sourced::new(value, ConfigSource::Environment)))
        .or_else(|| file.map(|value| Sourced::new(value, ConfigSource::File)))
}

fn parse_bitrate(value: &str) -> Result<u64, ConfigError> {
    let normalized = value.trim().to_ascii_lowercase();
    let (number, multiplier) = if let Some(number) = normalized.strip_suffix('k') {
        (number, 1_000_u64)
    } else if let Some(number) = normalized.strip_suffix('m') {
        (number, 1_000_000_u64)
    } else {
        (normalized.as_str(), 1_u64)
    };
    number
        .parse::<u64>()
        .ok()
        .and_then(|number| number.checked_mul(multiplier))
        .ok_or_else(|| ConfigError::InvalidValue {
            field: "bitrate",
            reason: format!("invalid bitrate `{value}`"),
        })
}

fn parse_layout(value: &str) -> Result<TargetLayout, ConfigError> {
    match value {
        "keep-up-to-5.1" => Ok(TargetLayout::KeepUpTo51),
        "stereo" => Ok(TargetLayout::Stereo),
        "5.1" => Ok(TargetLayout::Surround51),
        value => Err(ConfigError::InvalidValue {
            field: "channels",
            reason: format!("unsupported layout `{value}`"),
        }),
    }
}

fn parse_family(value: &str) -> Result<AudioCodecFamily, ConfigError> {
    match value {
        "ac3" => Ok(AudioCodecFamily::Ac3),
        "eac3" => Ok(AudioCodecFamily::Eac3),
        "aac" => Ok(AudioCodecFamily::Aac),
        "mp3" => Ok(AudioCodecFamily::Mp3),
        "dts" => Ok(AudioCodecFamily::Dts),
        "truehd" => Ok(AudioCodecFamily::TrueHd),
        "flac" => Ok(AudioCodecFamily::Flac),
        "opus" => Ok(AudioCodecFamily::Opus),
        "vorbis" => Ok(AudioCodecFamily::Vorbis),
        "pcm" => Ok(AudioCodecFamily::Pcm),
        value => Err(ConfigError::InvalidValue {
            field: "profile codec",
            reason: format!("unknown codec family `{value}`"),
        }),
    }
}

fn validate_profiles(profiles: &BTreeMap<String, ProfileFile>) -> Result<(), ConfigError> {
    for profile in profiles.values() {
        for (family, rule) in &profile.codecs {
            let _family = parse_family(family)?;
            if rule.maximum_channels == Some(0) {
                return Err(ConfigError::InvalidValue {
                    field: "maximum-channels",
                    reason: "must be greater than zero".to_owned(),
                });
            }
            if let Some(layouts) = &rule.allowed_layouts {
                if layouts.is_empty() || layouts.iter().any(|value| value.trim().is_empty()) {
                    return Err(ConfigError::InvalidValue {
                        field: "allowed-layouts",
                        reason: "must contain non-empty layout names".to_owned(),
                    });
                }
            }
        }
    }
    Ok(())
}

fn invalid_bitrate(error: impl std::fmt::Display) -> ConfigError {
    ConfigError::InvalidValue {
        field: "bitrate",
        reason: error.to_string(),
    }
}

fn collect_set(values: impl Iterator<Item = String>) -> std::collections::BTreeSet<String> {
    values.collect()
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::{
        ConfigPath, ConfigSource, DefaultConfig, PartialConfig, load_file, merge_config,
        parse_bitrate,
    };

    #[test]
    fn precedence_and_sources_are_retained() {
        let file = PartialConfig {
            codec: Some("aac".to_owned()),
            ..PartialConfig::default()
        };
        let environment = PartialConfig {
            codec: Some("eac3".to_owned()),
            bitrate: Some("512k".to_owned()),
            ..PartialConfig::default()
        };
        let cli = PartialConfig {
            codec: Some("ac3".to_owned()),
            ..PartialConfig::default()
        };
        let effective = merge_config(DefaultConfig::default(), file, environment, cli)
            .expect("valid merged config");
        assert_eq!(effective.codec.value(), "ac3");
        assert_eq!(effective.codec.source(), ConfigSource::Cli);
        assert_eq!(effective.bitrate.source(), ConfigSource::Environment);
    }

    #[test]
    fn bitrate_suffixes_are_decimal() {
        assert_eq!(parse_bitrate("640k").expect("valid"), 640_000);
        assert_eq!(parse_bitrate("1m").expect("valid"), 1_000_000);
    }

    #[test]
    fn strict_file_rejects_unknown_keys_and_builtin_profile_collisions() {
        let directory = tempdir().expect("temporary directory");
        let path = directory.path().join("config.toml");
        fs::write(&path, "version = 1\nmisspelled = true\n").expect("fixture writes");
        assert!(
            load_file(&ConfigPath {
                path: path.clone(),
                required: true
            })
            .is_err()
        );

        fs::write(
            &path,
            "version = 1\n[profiles.samsung]\nunknown-codec = \"reject\"\n",
        )
        .expect("fixture writes");
        assert!(
            load_file(&ConfigPath {
                path,
                required: true
            })
            .is_err()
        );
    }
}
