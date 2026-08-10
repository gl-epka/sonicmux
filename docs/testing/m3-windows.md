# M3 Windows manual validation

- Status: Pending
- Target: Windows 11 x86_64
- Milestone: M3

This is the first manual end-to-end platform check for SonicMux. Record the
machine, FFmpeg version, commit, operator, and date before changing the status to
`Passed`.

## Prerequisites

- the tested M3 commit is checked out with a clean worktree;
- stable Rust and the workspace MSRV toolchain are installed;
- native Windows `ffmpeg.exe` and `ffprobe.exe` from the selected hybrid
  distribution source are available;
- no unrelated FFmpeg process is running.

In PowerShell, point the gated test at the exact binaries and enable real-media
tests:

```powershell
$env:FFMPEG_PATH = (Resolve-Path 'C:\path\to\ffmpeg.exe').Path
$env:FFPROBE_PATH = (Resolve-Path 'C:\path\to\ffprobe.exe').Path
$env:SONICMUX_RUN_FFMPEG_TESTS = '1'
```

## Required checks

### 1. Successful atomic commit

```powershell
cargo test -p sonicmux-runtime --test ffmpeg_execution -- --nocapture
```

Pass when the generated DTS MKV is converted to AC-3, validation succeeds, the
ordered SHA-256 video packet hashes match, and the test directory contains no
`.sonicmux-*` staging directory afterward. No additional console window may
flash while FFmpeg or FFprobe runs.

### 2. Destination collision

```powershell
cargo test -p sonicmux-runtime execution::tests::competing_destination_is_never_overwritten -- --exact --nocapture
```

Pass when the competing destination retains its sentinel content, SonicMux
returns the typed collision error, and no staging directory remains.

### 3. Cancellation and process cleanup

```powershell
cargo test -p sonicmux-runtime execution::tests::backend_failure_and_cancellation_remove_staging -- --exact --nocapture
```

Pass when cancellation is reported only after the backend marks the operation
reaped and no staging directory remains. During the real-media run in check 1,
also cancel the test process once while FFmpeg is active; confirm in Task Manager
that no child `ffmpeg.exe` or `ffprobe.exe` remains after the test process exits.

The interrupted run is intentionally a destructive test-process cancellation,
not a product success case. Run check 1 again afterward to confirm a clean
successful transaction.

### 4. Complete Windows workspace check

```powershell
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets
cargo +1.85.0 check --workspace --all-targets
```

All commands must pass without a console-window flash or lingering FFmpeg
process.

## Result record

Fill this section only from the Windows machine:

```text
Status: Pending
Date:
Operator:
Commit:
Windows edition/build:
Architecture:
FFmpeg version/source:
FFprobe version/source:
Successful commit: [ ]
Destination collision: [ ]
Cancellation and no process leak: [ ]
No console-window flash: [ ]
Workspace checks: [ ]
Notes:
```
