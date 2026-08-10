//! Deterministic `JobPlan` to FFmpeg argument conversion.

use std::{ffi::OsString, path::Path};

use sonicmux_core::{AudioTarget, ExpectedStreamKind, JobPlan, OutputStreamPlan};
use thiserror::Error;

/// Failure while converting a typed plan into FFmpeg arguments.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ArgumentError {
    /// The plan contains an operation not supported by this adapter version.
    #[error("job plan contains an unsupported stream operation")]
    UnsupportedOperation,
    /// Stream operations and validation expectations lost their shared order.
    #[error("job plan stream operations and expectations are inconsistent")]
    InconsistentPlan,
    /// An encoder target is not supported by this adapter version.
    #[error("job plan contains an unsupported audio target")]
    UnsupportedAudioTarget,
}

/// FFmpeg arguments plus bounded non-fatal construction warnings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArgumentBuild {
    arguments: Vec<OsString>,
    warnings: Vec<String>,
}

impl ArgumentBuild {
    /// Returns the direct process argument array.
    #[must_use]
    pub fn arguments(&self) -> &[OsString] {
        &self.arguments
    }

    /// Returns construction warnings.
    #[must_use]
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    /// Consumes the build into its argument array and warnings.
    #[must_use]
    pub fn into_parts(self) -> (Vec<OsString>, Vec<String>) {
        (self.arguments, self.warnings)
    }
}

/// Builds a deterministic FFmpeg argument array without invoking a shell.
///
/// # Errors
///
/// Returns [`ArgumentError`] when the plan contains an unsupported operation or
/// its operation/expectation ordering is inconsistent.
pub fn build_execution_arguments(
    plan: &JobPlan,
    staging_path: &Path,
) -> Result<ArgumentBuild, ArgumentError> {
    if plan.streams().len() != plan.expected().streams().len() {
        return Err(ArgumentError::InconsistentPlan);
    }

    let mut arguments = vec![
        "-hide_banner".into(),
        "-nostdin".into(),
        "-y".into(),
        "-progress".into(),
        "pipe:1".into(),
        "-stats_period".into(),
        "0.25".into(),
        "-nostats".into(),
        "-copyts".into(),
        "-i".into(),
        plan.input().as_os_str().to_owned(),
        "-map_metadata".into(),
        if plan.copy_global_metadata() {
            "0".into()
        } else {
            "-1".into()
        },
        "-map_chapters".into(),
        if plan.copy_chapters() {
            "0".into()
        } else {
            "-1".into()
        },
        "-copy_unknown".into(),
    ];

    for operation in plan.streams() {
        arguments.push("-map".into());
        arguments.push(format!("0:{}", operation.source().get()).into());
    }
    arguments.extend([OsString::from("-c"), OsString::from("copy")]);

    let mut audio_ordinal = 0_usize;
    for (output_ordinal, (operation, expected)) in plan
        .streams()
        .iter()
        .zip(plan.expected().streams())
        .enumerate()
    {
        let is_audio = matches!(expected.kind(), ExpectedStreamKind::Audio);
        match operation {
            OutputStreamPlan::Copy { .. } => {}
            OutputStreamPlan::EncodeAudio {
                target,
                output_channels,
                metadata,
                ..
            } => {
                if !is_audio {
                    return Err(ArgumentError::InconsistentPlan);
                }
                let (encoder, bitrate) = encoder_and_bitrate(target)?;
                push_pair(&mut arguments, format!("-c:a:{audio_ordinal}"), encoder);
                push_pair(
                    &mut arguments,
                    format!("-b:a:{audio_ordinal}"),
                    bitrate.to_string(),
                );
                push_pair(
                    &mut arguments,
                    format!("-ac:a:{audio_ordinal}"),
                    output_channels.get().to_string(),
                );
                for key in ["language", "title"] {
                    if let Some(value) = metadata.metadata().get(key) {
                        push_pair(
                            &mut arguments,
                            format!("-metadata:s:{output_ordinal}"),
                            format!("{key}={value}"),
                        );
                    }
                }
            }
            _ => return Err(ArgumentError::UnsupportedOperation),
        }

        push_pair(
            &mut arguments,
            format!("-disposition:{output_ordinal}"),
            disposition_value(operation),
        );
        if is_audio {
            audio_ordinal += 1;
        }
    }

    arguments.extend([
        "-copytb".into(),
        "1".into(),
        "-avoid_negative_ts".into(),
        "disabled".into(),
        "-f".into(),
        "matroska".into(),
        staging_path.as_os_str().to_owned(),
    ]);
    Ok(ArgumentBuild {
        arguments,
        warnings: Vec::new(),
    })
}

fn push_pair(
    arguments: &mut Vec<OsString>,
    option: impl Into<OsString>,
    value: impl Into<OsString>,
) {
    arguments.push(option.into());
    arguments.push(value.into());
}

fn disposition_value(operation: &OutputStreamPlan) -> String {
    let enabled = operation
        .dispositions()
        .to_flags()
        .into_iter()
        .filter_map(|(name, value)| value.then_some(name))
        .collect::<Vec<_>>();
    if enabled.is_empty() {
        "0".to_owned()
    } else {
        enabled.join("+")
    }
}

fn encoder_and_bitrate(target: &AudioTarget) -> Result<(&'static str, u64), ArgumentError> {
    match target {
        AudioTarget::Ac3 { bitrate, .. } => Ok(("ac3", bitrate.get())),
        AudioTarget::Eac3 { bitrate, .. } => Ok(("eac3", bitrate.get())),
        AudioTarget::Aac { bitrate, .. } => Ok(("aac", bitrate.get())),
        _ => Err(ArgumentError::UnsupportedAudioTarget),
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests {
    use std::path::{Path, PathBuf};

    use insta::assert_debug_snapshot;
    use sonicmux_core::{
        AacBitrate, Ac3Bitrate, AudioSelector, AudioTarget, CompatibilityPolicy, Eac3Bitrate,
        OutputMode, PlanOutcome, PlanningPolicy, ProfileName, RequestedAction, TargetLayout, build,
    };

    use super::build_execution_arguments;
    use crate::parse_probe_output;

    fn add_plan() -> sonicmux_core::JobPlan {
        plan_with(
            OutputMode::Add,
            RequestedAction::Convert,
            AudioTarget::Ac3 {
                bitrate: Ac3Bitrate::new(640_000).expect("bitrate is valid"),
                layout: TargetLayout::KeepUpTo51,
            },
        )
    }

    fn plan_with(
        mode: OutputMode,
        action: RequestedAction,
        target: AudioTarget,
    ) -> sonicmux_core::JobPlan {
        let media = parse_probe_output(
            PathBuf::from("<INPUT>.mkv"),
            include_bytes!("../tests/fixtures/mixed.json"),
        )
        .expect("fixture parses");
        let policy = PlanningPolicy::new(
            CompatibilityPolicy::for_profile(ProfileName::GenericTv),
            target,
            mode,
            action,
            PathBuf::from("<OUTPUT>"),
        );
        match build(&media, &policy).expect("plan builds") {
            PlanOutcome::Execute(plan) => plan,
            PlanOutcome::Skip(reason) => panic!("unexpected skip: {reason:?}"),
            _ => panic!("unexpected future plan outcome"),
        }
    }

    #[test]
    fn add_arguments_are_ordered_and_shell_free() {
        let build = build_execution_arguments(&add_plan(), Path::new("<STAGING>"))
            .expect("arguments build");
        assert_debug_snapshot!("add_arguments", build.arguments());
    }

    fn snapshot_arguments(name: &str, plan: &sonicmux_core::JobPlan) {
        let build =
            build_execution_arguments(plan, Path::new("<STAGING>")).expect("arguments build");
        assert_debug_snapshot!(name, build.arguments());
    }

    #[test]
    fn snapshot_replace_arguments() {
        snapshot_arguments(
            "replace_arguments",
            &plan_with(
                OutputMode::Replace,
                RequestedAction::Convert,
                AudioTarget::Ac3 {
                    bitrate: Ac3Bitrate::new(640_000).expect("bitrate is valid"),
                    layout: TargetLayout::KeepUpTo51,
                },
            ),
        );
    }

    #[test]
    fn snapshot_only_new_arguments() {
        snapshot_arguments(
            "only_new_arguments",
            &plan_with(
                OutputMode::OnlyNew,
                RequestedAction::Convert,
                AudioTarget::Ac3 {
                    bitrate: Ac3Bitrate::new(640_000).expect("bitrate is valid"),
                    layout: TargetLayout::KeepUpTo51,
                },
            ),
        );
    }

    #[test]
    fn snapshot_remux_arguments() {
        snapshot_arguments(
            "remux_arguments",
            &plan_with(
                OutputMode::Add,
                RequestedAction::RemuxOnly {
                    selection: AudioSelector::FirstCompatible,
                },
                AudioTarget::Ac3 {
                    bitrate: Ac3Bitrate::new(640_000).expect("bitrate is valid"),
                    layout: TargetLayout::KeepUpTo51,
                },
            ),
        );
    }

    #[test]
    fn snapshot_stereo_arguments() {
        snapshot_arguments(
            "stereo_arguments",
            &plan_with(
                OutputMode::Replace,
                RequestedAction::Convert,
                AudioTarget::Ac3 {
                    bitrate: Ac3Bitrate::new(448_000).expect("bitrate is valid"),
                    layout: TargetLayout::Stereo,
                },
            ),
        );
    }

    #[test]
    fn snapshot_aac_arguments() {
        snapshot_arguments(
            "aac_arguments",
            &plan_with(
                OutputMode::Replace,
                RequestedAction::Convert,
                AudioTarget::Aac {
                    bitrate: AacBitrate::new(320_000).expect("bitrate is valid"),
                    layout: TargetLayout::Stereo,
                },
            ),
        );
    }

    #[test]
    fn snapshot_eac3_arguments() {
        snapshot_arguments(
            "eac3_arguments",
            &plan_with(
                OutputMode::Replace,
                RequestedAction::Convert,
                AudioTarget::Eac3 {
                    bitrate: Eac3Bitrate::new(768_000).expect("bitrate is valid"),
                    layout: TargetLayout::Surround51,
                },
            ),
        );
    }

    #[test]
    fn snapshot_explicit_remux_selection_arguments() {
        snapshot_arguments(
            "explicit_remux_selection_arguments",
            &plan_with(
                OutputMode::Add,
                RequestedAction::RemuxOnly {
                    selection: AudioSelector::StreamIndex(sonicmux_core::StreamIndex::new(2)),
                },
                AudioTarget::Ac3 {
                    bitrate: Ac3Bitrate::new(640_000).expect("bitrate is valid"),
                    layout: TargetLayout::KeepUpTo51,
                },
            ),
        );
    }

    #[test]
    fn every_output_stream_has_an_explicit_disposition() {
        let plan = add_plan();
        let build =
            build_execution_arguments(&plan, Path::new("<STAGING>")).expect("arguments build");
        let disposition_count = build
            .arguments()
            .iter()
            .filter(|argument| argument.to_string_lossy().starts_with("-disposition:"))
            .count();
        assert_eq!(disposition_count, plan.streams().len());
    }

    #[test]
    fn encoded_audio_uses_its_audio_output_ordinal() {
        let build = build_execution_arguments(&add_plan(), Path::new("<STAGING>"))
            .expect("arguments build");
        assert!(
            build
                .arguments()
                .iter()
                .any(|argument| argument == "-c:a:2")
        );
        assert!(
            !build
                .arguments()
                .iter()
                .any(|argument| argument == "-c:a:5")
        );
    }
}
