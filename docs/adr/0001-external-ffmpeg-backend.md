# ADR-0001: Use external FFmpeg processes behind a backend interface

- Status: Accepted
- Date: 2026-08-10

## Context

SonicMux must inspect Matroska files, decode DTS, DTS-HD MA, and TrueHD, encode
AC-3/E-AC-3/AAC, and remux every other stream without re-encoding. It must work
on Linux, Windows, and macOS on both x86_64 and aarch64.

The media implementation must not leak into planning or user interfaces. Tests
of compatibility and stream mapping must run without FFmpeg or media files.

Three implementation approaches were considered.

### External `ffmpeg` and `ffprobe` processes

The application invokes `ffprobe` with JSON output and invokes `ffmpeg` with
machine-readable progress output.

Advantages:

- no C toolchain or libav ABI coupling in the Rust build;
- FFmpeg can be updated independently;
- decoding and encoding support matches the installed or bundled build;
- cross-compilation and packaging remain tractable;
- SonicMux does not link to FFmpeg libraries.

Disadvantages:

- executable discovery and version/capability checks are required;
- child-process lifecycle, cancellation, stderr capture, and cleanup must be
  implemented carefully;
- JSON and progress output are external protocols that need fixture tests;
- a system FFmpeg can differ between machines.

### `ffmpeg-next` or `rsmpeg`

The application links against libav libraries.

Advantages:

- direct access to streams, timestamps, errors, and progress;
- no text command construction or child-process protocol;
- more control over unusual containers and codecs.

Disadvantages:

- substantially harder cross-platform builds and CI;
- libav version/ABI compatibility and native dependency discovery;
- unsafe FFI is unavoidable below the safe wrapper;
- LGPL/GPL linking and distribution obligations become part of the product;
- a backend crash can terminate the entire application.

### Pure Rust media stack

Crates such as `symphonia` and Matroska parsers could provide parts of the
pipeline.

Advantages:

- one Rust dependency graph and no external executable;
- maximal control over safety and portability;
- potentially easier static distribution after codec coverage exists.

Disadvantages:

- DTS-HD MA and TrueHD coverage is not sufficient for this product;
- there is no production-ready pure-Rust AC-3 encoder matching the requirement;
- implementing codec and muxing gaps would become a separate multi-year project.

## Decision

Use external `ffprobe` and `ffmpeg` processes for the first production backend.
The adapter lives in `sonicmux-ffmpeg`, not in `sonicmux-core`.

`ffprobe` will be invoked with a stable JSON-oriented contract equivalent to:

```text
ffprobe -v error -print_format json -show_streams -show_format -show_chapters INPUT
```

FFmpeg execution will use `-progress pipe:1 -nostats`. Standard output is
reserved for progress records; standard error is captured into a bounded
diagnostic tail and optionally streamed to structured logs. Arguments are
passed directly to the process API, never through a shell.

The application-level interface is execution-oriented rather than named only
for transcoding, because a plan can be a probe, remux, or transcode:

```rust
#[async_trait]
pub trait MediaBackend: Send + Sync {
    async fn probe(&self, path: &Path) -> Result<MediaInfo, BackendError>;

    async fn execute(
        &self,
        plan: &JobPlan,
        progress: mpsc::Sender<ProgressEvent>,
        cancel: CancellationToken,
    ) -> Result<JobReport, BackendError>;

    async fn validate(&self, path: &Path) -> Result<MediaInfo, BackendError>;
}
```

This trait belongs to the application/runtime boundary. `sonicmux-core` owns the
domain types and pure planner but has no dependency on async runtimes, process
APIs, FFmpeg, or this trait. `MockBackend` is supplied by runtime test support.

The implementation must probe FFmpeg at startup and report its version and the
availability of the selected decoders, encoders, and Matroska muxer. A binary
being present is not considered sufficient.

## Consequences

- Initial development and releases are feasible on all target platforms.
- Runtime behavior depends on the capabilities of a system or bundled FFmpeg.
- Command argument generation, ffprobe parsing, and progress parsing become
  explicit, snapshot-tested protocols.
- Cancellation must terminate the process tree on each supported OS and wait for
  it to exit before temporary-file cleanup.
- A future `sonicmux-ffmpeg-lib` adapter can implement the same application
  interface behind an additive Cargo feature without changing the planner or UI.
- Pure Rust is not a roadmap commitment; it can be reconsidered only after the
  required decoder, encoder, and Matroska capabilities exist.
