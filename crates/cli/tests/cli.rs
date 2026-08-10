#![doc = "Black-box command-line behavior and snapshot tests."]
#![allow(clippy::expect_used, clippy::panic)]

use assert_cmd::cargo::cargo_bin_cmd;
use predicates::prelude::*;

#[test]
fn command_snapshots() {
    trycmd::TestCases::new().case("tests/cmd/*.trycmd");
}

#[test]
fn config_init_is_create_new_and_json_is_versioned() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("sonicmux.toml");

    cargo_bin_cmd!("sonicmux")
        .args([
            "--config",
            path.to_str().expect("Unicode temp path"),
            "config",
            "init",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Created"));
    cargo_bin_cmd!("sonicmux")
        .args([
            "--config",
            path.to_str().expect("Unicode temp path"),
            "config",
            "init",
        ])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("failed to create configuration"));
    let output = cargo_bin_cmd!("sonicmux")
        .args([
            "--config",
            path.to_str().expect("Unicode temp path"),
            "--json",
            "config",
            "show",
        ])
        .env("SONICMUX_JOBS", "2")
        .output()
        .expect("config show launches");
    assert!(output.status.success());
    let actual: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("machine output parses");
    let expected: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/config-result-v1.json"))
            .expect("checked-in schema fixture parses");
    assert_eq!(actual, expected);
}

#[test]
fn artifact_commands_do_not_require_config_or_ffmpeg() {
    cargo_bin_cmd!("sonicmux")
        .args(["completions", "bash"])
        .env("PATH", "")
        .assert()
        .success()
        .stdout(predicate::str::contains("_sonicmux"));
    cargo_bin_cmd!("sonicmux")
        .arg("man")
        .env("PATH", "")
        .assert()
        .success()
        .stdout(predicate::str::contains(".TH sonicmux 1"));
}

#[test]
fn machine_mode_conflicts_are_usage_errors() {
    cargo_bin_cmd!("sonicmux")
        .args(["--json-progress", "config", "path"])
        .assert()
        .code(2)
        .stdout(predicate::str::contains("\"event\":\"batch_failed\""));
    cargo_bin_cmd!("sonicmux")
        .args(["--json", "completions", "zsh"])
        .assert()
        .code(2)
        .stdout(predicate::str::contains("\"schema\":\"sonicmux.result\""));
}

#[test]
fn explicit_color_wins_and_no_color_disables_auto() {
    cargo_bin_cmd!("sonicmux")
        .args(["--color", "always", "config", "path"])
        .env("NO_COLOR", "1")
        .assert()
        .success()
        .stdout(predicate::str::contains("\u{1b}[32m"));
    cargo_bin_cmd!("sonicmux")
        .args(["config", "path"])
        .env("NO_COLOR", "1")
        .assert()
        .success()
        .stdout(predicate::str::contains("\u{1b}[").not());
}

#[test]
fn scheduler_arguments_are_bounded_and_failure_flags_conflict() {
    cargo_bin_cmd!("sonicmux")
        .args(["convert", "movie.mkv", "--jobs", "0"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("1 through 64"));
    cargo_bin_cmd!("sonicmux")
        .args(["convert", "movie.mkv", "--continue-on-error", "--fail-fast"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("cannot be used with"));
    cargo_bin_cmd!("sonicmux")
        .args(["--json", "config", "show"])
        .env("SONICMUX_JOBS", "65")
        .assert()
        .code(2)
        .stdout(predicate::str::contains("\"exit_code\":2"));
}
