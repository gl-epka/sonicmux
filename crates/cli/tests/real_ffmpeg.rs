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

#[test]
fn two_generated_inputs_convert_in_one_parallel_batch() {
    if std::env::var_os("SONICMUX_RUN_FFMPEG_TESTS").as_deref() != Some(std::ffi::OsStr::new("1")) {
        return;
    }
    let directory = tempfile::tempdir().expect("temporary directory");
    let output_directory = directory.path().join("outputs");
    std::fs::create_dir(&output_directory).expect("output directory creates");
    let first = directory.path().join("first.mkv");
    let second = directory.path().join("second.mkv");
    if let Some(source) = std::env::var_os("SONICMUX_CLI_FIXTURE") {
        std::fs::copy(&source, &first).expect("first fixture copies");
        std::fs::copy(source, &second).expect("second fixture copies");
    } else {
        let generator = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("testdata/generate-m3-fixture.sh");
        for input in [&first, &second] {
            let status = Command::new("sh")
                .arg(&generator)
                .arg(input)
                .status()
                .expect("fixture generator launches");
            assert!(status.success());
        }
    }
    let dry_run = cargo_bin_cmd!("sonicmux")
        .args(["--json-progress", "convert"])
        .args([&first, &second])
        .args(["--jobs", "2", "--dry-run", "--output-dir"])
        .arg(&output_directory)
        .output()
        .expect("parallel dry-run launches");
    assert!(dry_run.status.success());
    let events: Vec<serde_json::Value> = String::from_utf8(dry_run.stdout)
        .expect("NDJSON is UTF-8")
        .lines()
        .map(|line| serde_json::from_str(line).expect("NDJSON event parses"))
        .collect();
    assert!(events.iter().enumerate().all(|(sequence, event)| {
        event["schema"] == "sonicmux.event" && event["sequence"] == sequence
    }));
    assert_eq!(events[0]["event"], "batch_started");
    assert_eq!(events[0]["data"]["jobs"], 2);
    assert_eq!(events[0]["data"]["storage_profile"], "balanced");
    assert_eq!(
        events.last().expect("terminal event exists")["event"],
        "batch_finished"
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event["event"] == "file_succeeded")
            .count(),
        2
    );
    let conversion = cargo_bin_cmd!("sonicmux")
        .args(["--json", "convert"])
        .args([&first, &second])
        .args(["--jobs", "2", "--output-dir"])
        .arg(&output_directory)
        .output()
        .expect("parallel conversion launches");
    assert!(conversion.status.success());
    let value: serde_json::Value =
        serde_json::from_slice(&conversion.stdout).expect("batch JSON parses");
    let files = value["data"]["files"]
        .as_array()
        .expect("batch files array exists");
    assert_eq!(files.len(), 2);
    assert!(files.iter().all(|file| file["status"] == "success"));
    assert!(output_directory.join("first.sonicmux.mkv").is_file());
    assert!(output_directory.join("second.sonicmux.mkv").is_file());
}

#[cfg(unix)]
#[test]
fn cancellation_reaps_process_group_and_removes_staging() {
    use std::{
        fs,
        os::unix::fs::{PermissionsExt as _, symlink},
        process::Stdio,
        thread,
        time::{Duration, Instant},
    };

    if std::env::var_os("SONICMUX_RUN_FFMPEG_TESTS").as_deref() != Some(std::ffi::OsStr::new("1")) {
        return;
    }
    let directory = tempfile::tempdir().expect("temporary directory");
    let tools = directory.path().join("tools");
    fs::create_dir(&tools).expect("tool directory creates");
    let real_ffmpeg = which::which("ffmpeg").expect("system FFmpeg is installed");
    let real_ffprobe = which::which("ffprobe").expect("system FFprobe is installed");
    symlink(real_ffmpeg, tools.join("ffmpeg.real")).expect("real FFmpeg link creates");
    symlink(real_ffprobe, tools.join("ffprobe")).expect("FFprobe link creates");
    let wrapper = tools.join("ffmpeg");
    fs::write(
        &wrapper,
        "#!/bin/sh\nprintf '%s\\n' \"$$\" > \"$SONICMUX_TEST_PID_FILE\"\nsleep 30\nexec \"$(dirname \"$0\")/ffmpeg.real\" \"$@\"\n",
    )
    .expect("FFmpeg wrapper writes");
    let mut permissions = fs::metadata(&wrapper)
        .expect("wrapper metadata reads")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&wrapper, permissions).expect("wrapper becomes executable");

    let input = directory.path().join("cancel input.mkv");
    let output = directory.path().join("cancel input.sonicmux.mkv");
    if let Some(source) = std::env::var_os("SONICMUX_CLI_FIXTURE") {
        fs::copy(source, &input).expect("pre-generated fixture copies");
    } else {
        let generator = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("testdata/generate-m3-fixture.sh");
        let status = Command::new("sh")
            .arg(generator)
            .arg(&input)
            .status()
            .expect("fixture generator launches");
        assert!(status.success());
    }
    let pid_file = directory.path().join("ffmpeg.pid");
    let mut child = Command::new(assert_cmd::cargo::cargo_bin!("sonicmux"))
        .arg("--ffmpeg-path")
        .arg(&tools)
        .arg("convert")
        .arg(&input)
        .env("SONICMUX_TEST_PID_FILE", &pid_file)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("SonicMux launches");
    let deadline = Instant::now() + Duration::from_secs(10);
    while !pid_file.is_file() {
        assert!(Instant::now() < deadline, "FFmpeg wrapper did not start");
        thread::sleep(Duration::from_millis(10));
    }
    let ffmpeg_pid = fs::read_to_string(&pid_file)
        .expect("PID file reads")
        .trim()
        .parse::<u32>()
        .expect("wrapper PID parses");
    let interrupt = Command::new("kill")
        .args(["-INT", &child.id().to_string()])
        .status()
        .expect("interrupt command launches");
    assert!(interrupt.success());
    let status = loop {
        if let Some(status) = child.try_wait().expect("CLI status polls") {
            break status;
        }
        if Instant::now() >= deadline {
            child.kill().expect("timed-out CLI is terminated");
            panic!("SonicMux did not finish cancellation");
        }
        thread::sleep(Duration::from_millis(10));
    };
    assert_eq!(status.code(), Some(130));
    assert!(!output.exists());
    assert!(
        !fs::read_dir(directory.path())
            .expect("temporary directory is readable")
            .filter_map(Result::ok)
            .any(|entry| entry
                .file_name()
                .to_string_lossy()
                .starts_with(".sonicmux-"))
    );
    let group_deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let output = Command::new("ps")
            .args(["-eo", "pgid="])
            .output()
            .expect("process table reads");
        let group_exists = String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(|line| line.trim().parse::<u32>().ok())
            .any(|group| group == ffmpeg_pid);
        if !group_exists {
            break;
        }
        assert!(
            Instant::now() < group_deadline,
            "FFmpeg process group remained alive after cancellation"
        );
        thread::sleep(Duration::from_millis(10));
    }
}
