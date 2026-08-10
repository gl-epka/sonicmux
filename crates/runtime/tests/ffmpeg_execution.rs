//! Gated real-FFmpeg execution test with a generated DTS fixture.
#![allow(clippy::expect_used, clippy::panic)]

use std::{env, ffi::OsString, path::Path, sync::Arc};

use serde_json::Value;
use sonicmux_core::{
    Ac3Bitrate, AudioCodec, AudioTarget, CompatibilityPolicy, OutputMode, PlanOutcome,
    PlanningPolicy, ProfileName, RequestedAction, TargetLayout, build,
};
use sonicmux_ffmpeg::{FfmpegCliBackend, FfmpegToolchainPaths};
use sonicmux_runtime::execute_safely;
use tempfile::TempDir;
use tokio::{process::Command, sync::mpsc};
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn generated_dts_conversion_preserves_video_packet_payloads() {
    if env::var_os("SONICMUX_RUN_FFMPEG_TESTS").as_deref() != Some("1".as_ref()) {
        eprintln!("skipping real FFmpeg test; set SONICMUX_RUN_FFMPEG_TESTS=1 to enable");
        return;
    }

    let ffmpeg = env::var_os("FFMPEG_PATH").unwrap_or_else(|| OsString::from("ffmpeg"));
    let ffprobe = env::var_os("FFPROBE_PATH").unwrap_or_else(|| OsString::from("ffprobe"));
    let directory = TempDir::new().expect("temporary fixture directory creates");
    let input = directory.path().join("fixture.mkv");
    let output = directory.path().join("fixture.sonicmux.mkv");
    let metadata = directory.path().join("chapters.ffmetadata");
    std::fs::write(
        &metadata,
        ";FFMETADATA1\ntitle=SonicMux M3 Fixture\n[CHAPTER]\nTIMEBASE=1/1000\nSTART=0\nEND=2500\ntitle=First half\n[CHAPTER]\nTIMEBASE=1/1000\nSTART=2500\nEND=5000\ntitle=Second half\n",
    )
    .expect("chapter metadata writes");
    generate_fixture(&ffmpeg, &metadata, &input).await;

    let backend = FfmpegCliBackend::new(FfmpegToolchainPaths::new(
        ffmpeg.into(),
        ffprobe.clone().into(),
    ));
    let media = backend.probe(&input).await.expect("input probes");
    let policy = PlanningPolicy::new(
        CompatibilityPolicy::for_profile(ProfileName::GenericTv),
        AudioTarget::Ac3 {
            bitrate: Ac3Bitrate::new(640_000).expect("AC-3 bitrate is valid"),
            layout: TargetLayout::KeepUpTo51,
        },
        OutputMode::Add,
        RequestedAction::Convert,
        output.clone(),
    );
    let plan = match build(&media, &policy).expect("DTS plan builds") {
        PlanOutcome::Execute(plan) => Arc::new(plan),
        PlanOutcome::Skip(reason) => panic!("unexpected skip: {reason:?}"),
        _ => panic!("unexpected future plan outcome"),
    };
    let (progress, mut progress_receiver) = mpsc::channel(16);
    let report = execute_safely(
        &backend,
        Arc::clone(&plan),
        progress,
        CancellationToken::new(),
    )
    .await
    .expect("safe execution succeeds");
    assert_eq!(report.output(), output);
    while progress_receiver.try_recv().is_ok() {}

    let converted = backend.probe(&output).await.expect("output probes");
    let audio_codecs = converted
        .audio_streams()
        .map(|stream| stream.codec())
        .collect::<Vec<_>>();
    assert!(matches!(
        audio_codecs.as_slice(),
        [AudioCodec::Dts(_), AudioCodec::Ac3]
    ));
    assert_eq!(converted.chapters().len(), 2);

    let input_hashes = packet_hashes(&ffprobe, &input).await;
    let output_hashes = packet_hashes(&ffprobe, &output).await;
    assert!(!input_hashes.is_empty());
    assert_eq!(input_hashes, output_hashes);
    assert_no_staging(directory.path());
}

async fn generate_fixture(ffmpeg: &OsString, metadata: &Path, output: &Path) {
    let ffmpeg = ffmpeg.clone();
    let metadata = metadata.to_path_buf();
    let output = output.to_path_buf();
    let status = tokio::task::spawn_blocking(move || {
        std::process::Command::new(ffmpeg)
            .args([
                "-v",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=160x90:rate=25:duration=5",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=1000:sample_rate=48000:duration=5",
                "-f",
                "ffmetadata",
                "-i",
            ])
            .arg(&metadata)
            .args([
                "-map",
                "0:v:0",
                "-map",
                "1:a:0",
                "-map_metadata",
                "2",
                "-map_chapters",
                "2",
                "-c:v",
                "mpeg4",
                "-q:v",
                "8",
                "-c:a",
                "dca",
                "-strict",
                "experimental",
                "-b:a",
                "768k",
                "-ac",
                "6",
                "-metadata:s:a:0",
                "language=eng",
                "-metadata:s:a:0",
                "title=Main",
                "-disposition:a:0",
                "default",
                "-shortest",
                "-f",
                "matroska",
            ])
            .arg(&output)
            .output()
    })
    .await
    .expect("fixture generator task joins")
    .expect("FFmpeg fixture generator launches");
    assert!(
        status.status.success(),
        "FFmpeg fixture generation failed with {:?}: {}",
        status.status,
        String::from_utf8_lossy(&status.stderr)
    );
}

async fn packet_hashes(ffprobe: &OsString, path: &Path) -> Vec<String> {
    let output = Command::new(ffprobe)
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_packets",
            "-show_entries",
            "packet=data_hash",
            "-show_data_hash",
            "sha256",
            "-of",
            "json",
        ])
        .arg(path)
        .output()
        .await
        .expect("FFprobe packet hash launches");
    assert!(output.status.success(), "FFprobe packet hash succeeds");
    let document: Value = serde_json::from_slice(&output.stdout).expect("packet JSON parses");
    document
        .get("packets")
        .and_then(Value::as_array)
        .expect("packet list exists")
        .iter()
        .map(|packet| {
            packet
                .get("data_hash")
                .and_then(Value::as_str)
                .expect("packet hash exists")
                .to_owned()
        })
        .collect()
}

fn assert_no_staging(parent: &Path) {
    let count = std::fs::read_dir(parent)
        .expect("fixture directory reads")
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with(".sonicmux-")
        })
        .count();
    assert_eq!(count, 0);
}
