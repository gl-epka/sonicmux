# Changelog

All notable changes to SonicMux are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0] - 2026-08-11

### Added

- Pure compatibility policy and deterministic MKV conversion planning.
- Safe external FFmpeg execution with atomic non-replacing publication.
- CLI probing, scanning, conversion, dry runs, JSON output, completions, and man pages.
- Bounded multi-file scheduling, progress aggregation, cancellation, and recovery.
- Keyboard-first terminal interface and cross-platform Tauri desktop interface.
- Configured, bundled-sidecar, then system FFmpeg discovery for the desktop app.
- Reproducible multi-platform packages, release provenance, and security automation.

### Security

- Rust-owned GUI path grants and named Tauri capabilities replace raw path IPC.
- FFmpeg child processes are isolated, reaped on cancellation, and never receive final output paths.

[Unreleased]: https://github.com/gl-epka/sonicmux/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/gl-epka/sonicmux/releases/tag/v0.1.0
