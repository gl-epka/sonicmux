# ADR-0004: Concurrency is bounded at the file level

- Status: Accepted
- Date: 2026-08-10

## Context

Audio encoding uses CPU, and FFmpeg can use multiple threads internally. For a
large MKV, however, SonicMux must still read and rewrite the video, subtitles,
attachments, chapters, and metadata. Copying tens of gigabytes is commonly the
dominant cost.

Unbounded parallel jobs can reduce throughput on HDDs, saturate memory and I/O,
and make the UI unresponsive. Parallelizing work within a single file can also
force extra passes over the same container.

The application must support cancellation, per-file and aggregate progress,
partial batch failures, and cleanup of temporary outputs.

## Decision

Use a Tokio application runtime and schedule complete files as the unit of
concurrency. A `Semaphore` caps active jobs and a `JoinSet` owns job tasks.
FFmpeg controls codec-level threading inside each process. Rayon is not included
until a measured, in-process CPU workload requires it.

Default concurrency is computed as:

```text
min(max(1, logical_cpu_count / 2), 4)
```

The storage profile can override that default:

- `hdd`: one active file;
- `balanced`: computed default;
- `nvme`: computed default, still capped unless `--jobs` is explicit.

SonicMux does not attempt unreliable automatic HDD/NVMe detection in the first
release. `--jobs N` has final precedence and is validated as a non-zero bounded
value.

Workers send bounded `ProgressEvent` values through `mpsc` to one aggregator.
The aggregator maintains snapshots in `watch` and emits user-facing events over
`broadcast`. CLI, TUI, and GUI subscribe; none invokes backend work synchronously
from its render/event loop.

Each job gets a child `CancellationToken`. Global cancellation stops admission
of new work, cancels running children, terminates their process trees, waits for
exit, and then removes registered temporary files. Dropping a task is not a
cancellation mechanism.

A failed file produces a `JobReport::Failed` and does not cancel unrelated files.
The batch report records succeeded, skipped, cancelled, and failed jobs and maps
them to documented CLI exit codes.

Progress is based on FFmpeg `out_time` divided by probed duration, clamped for
display but retains raw values for diagnostics. Aggregate progress is weighted
by duration initially. Byte-weighting may replace it if measurements show that
duration produces misleading results for stream-copy-heavy jobs. ETA is omitted
until enough samples exist and marked unknown for indeterminate inputs.

Before publishing performance claims, measure one versus multiple jobs with
`hyperfine` using the same representative files on HDD and NVMe. The README must
include hardware, FFmpeg version, input sizes/codecs, command, warmup policy, and
results. No unmeasured speedup claim is allowed.

## Consequences

- Parallel batches can use fast storage without multiplying work within a file.
- HDD users have a predictable safe profile.
- The event pipeline supports all three interfaces without coupling workers to
  presentation code.
- Process-tree termination needs platform-specific integration tests,
  particularly on Windows.
- Progress and ETA are estimates and must be labelled accordingly.
- Benchmark fixtures and methodology become release documentation work, not a
  unit-test responsibility.
