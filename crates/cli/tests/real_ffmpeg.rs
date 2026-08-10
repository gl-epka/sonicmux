#![doc = "Gated end-to-end CLI test using a generated DTS Matroska fixture."]
#![allow(clippy::expect_used, clippy::panic)]

use std::{path::PathBuf, process::Command};

use assert_cmd::cargo::cargo_bin_cmd;

#[test]
fn generated_dts_fixture_converts_through_public_cli() {
    if std::env::var_os("SONICMUX_RUN_FFMPEG_TESTS").as_deref() != Some(std::ffi::OsStr::new("1")) {
        return;
    }
    let directory = tempfile::tempdir().expect("temporary directory");
    let input = directory.path().join("input movie.mkv");
    let output = directory.path().join("input movie.sonicmux.mkv");
    let generator = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("testdata/generate-m3-fixture.sh");
    if let Some(source) = std::env::var_os("SONICMUX_CLI_FIXTURE") {
        std::fs::copy(source, &input).expect("pre-generated fixture copies");
    } else {
        let status = Command::new("sh")
            .arg(generator)
            .arg(&input)
            .status()
            .expect("fixture generator launches");
        assert!(status.success());
    }

    let dry_run = cargo_bin_cmd!("sonicmux")
        .args(["--json", "convert"])
        .arg(&input)
        .arg("--dry-run")
        .output()
        .expect("dry-run launches");
    assert!(dry_run.status.success());
    assert!(!output.exists());
    let dry_run_json: serde_json::Value =
        serde_json::from_slice(&dry_run.stdout).expect("dry-run JSON parses");
    assert_eq!(dry_run_json["schema"], "sonicmux.result");

    let conversion = cargo_bin_cmd!("sonicmux")
        .args(["--json", "convert"])
        .arg(&input)
        .output()
        .expect("conversion launches");
    assert!(conversion.status.success());
    assert!(output.is_file());
    let conversion_json: serde_json::Value =
        serde_json::from_slice(&conversion.stdout).expect("conversion JSON parses");
    assert_eq!(conversion_json["status"], "success");

    let probe = cargo_bin_cmd!("sonicmux")
        .args(["--json", "probe"])
        .arg(&output)
        .output()
        .expect("output probe launches");
    assert!(probe.status.success());
    let probe_json: serde_json::Value =
        serde_json::from_slice(&probe.stdout).expect("probe JSON parses");
    let streams = probe_json["data"]["files"][0]["media"]["streams"]
        .as_array()
        .expect("stream array exists");
    assert!(streams.iter().any(|stream| stream["codec"] == "AC-3"));
    assert!(
        !std::fs::read_dir(directory.path())
            .expect("directory can be read")
            .filter_map(Result::ok)
            .any(|entry| entry
                .file_name()
                .to_string_lossy()
                .starts_with(".sonicmux-"))
    );
}
