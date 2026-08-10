# M5 design: bounded parallel scheduler

- Status: Accepted
- Date: 2026-08-11
- Milestone: M5

## Scope

M5 replaces the sequential `convert` loop with a reusable, bounded file-level
scheduler in `sonicmux-runtime`. It adds parallel preparation and execution,
aggregate progress and ETA, explicit failure policy, and orderly batch
cancellation. One failed file does not stop the rest of the batch by default.

This design implements ADR-0004. A complete MKV file remains the unit of
concurrency. FFmpeg retains control of threads inside one process; SonicMux does
not add Rayon or split a file into multiple passes.

M5 includes:

1. a transport-neutral runtime batch API shared by CLI, TUI, and GUI;
2. bounded concurrent probe/plan preflight followed by bounded execution;
3. duration-weighted aggregate progress, per-file progress, speed, and ETA;
4. deterministic reports despite out-of-order completion;
5. continue-on-error and opt-in fail-fast behavior;
6. cancellation that waits for process-tree termination and staging cleanup;
7. `--jobs`, storage profiles, config/env precedence, and multi-progress CLI UX;
8. scheduler, cancellation, process cleanup, and command-contract tests;
9. a reproducible `hyperfine` benchmark and a measured README table.

M5 does not add TUI screens, GUI code, in-place replacement, overwrite, remote
inputs, non-MKV containers, automatic disk-type detection, distributed jobs, or
parallel transcoding within one file. `probe` and `scan` keep their M4 command
behavior; the reusable batch API is introduced first for `convert`, where
concurrency materially changes throughput and safety.

## User-visible command contract

`convert` gains four options:

```text
      --jobs <N>
          Maximum files processed at once [1..=64]

      --storage-profile <PROFILE>
          Storage concurrency profile [possible values: hdd, balanced, nvme]

      --continue-on-error
          Continue the batch after a file fails (default)

      --fail-fast
          Stop admission and cancel active files after the first failure
```

`--continue-on-error` and `--fail-fast` conflict. The explicit continue flag is
accepted for readable scripts even though it names the default. Both flags are
valid for `--dry-run`; fail-fast then applies to preparation failures and no
output is ever created.

The resolved maximum is:

```text
--jobs
> SONICMUX_JOBS
> [scheduler].jobs
> selected storage-profile default
```

The storage profile is resolved independently:

```text
--storage-profile
> SONICMUX_STORAGE_PROFILE
> [scheduler].storage-profile
> balanced
```

The computed default is
`min(max(1, available_parallelism / 2), 4)`. `hdd` forces one job when no
explicit job count wins. `balanced` and `nvme` use the computed default in M5,
as required by ADR-0004; `nvme` is an explicit intent marker for benchmark and
future tuning, not an unmeasured promise of higher concurrency. `--jobs` always
wins over the profile. Zero, values above 64, malformed environment values, and
unknown profile names are configuration errors with exit code 2.

The version-1 TOML file gains an additive strict section:

```toml
[scheduler]
# jobs = 2
storage-profile = "balanced"
```

`sonicmux config show --sources` reports the effective values and provenance.
`config init` documents the section. Failure policy remains per invocation and
is not persisted, so a config file cannot unexpectedly turn a resilient batch
into fail-fast mode.

## Runtime ownership boundary

`sonicmux-runtime` owns orchestration, because future interfaces must observe
the same queue, cancellation, and result semantics. The CLI owns only argument
parsing and rendering. `sonicmux-core` remains pure, and `sonicmux-backend`
continues to describe one backend operation rather than a batch.

The M4 `analyze` flow moves from the CLI into a typed runtime preparation API.
This prevents the TUI and GUI from reimplementing probe, remux selector
resolution, planning, existing-output inspection, and skip behavior.

Representative public types are:

```rust
pub struct BatchRequest {
    pub jobs: Vec<FileRequest>,
    pub options: SchedulerOptions,
}

pub struct FileRequest {
    pub id: JobId,
    pub input: PathBuf,
    pub output: PathBuf,
    pub compatibility: Arc<CompatibilityPolicy>,
    pub target: AudioTarget,
    pub output_mode: OutputMode,
    pub action: ActionRequest,
}

pub enum ActionRequest {
    Convert,
    RemuxOnly(AudioSelectionRequest),
}

pub enum AudioSelectionRequest {
    FirstCompatible,
    StreamIndex(StreamIndex),
    Language(String),
}

pub struct SchedulerOptions {
    pub max_concurrency: NonZeroUsize,
    pub failure_policy: FailurePolicy,
    pub dry_run: bool,
}

pub enum FailurePolicy {
    Continue,
    FailFast,
}
```

These are sketches, not a promise to expose fields directly. Constructors keep
invalid values unrepresentable. `JobId` is a stable monotonically assigned
ordinal in discovery order, not a path hash and not a process ID.

`Runtime::start_batch` returns a handle immediately:

```rust
pub struct BatchHandle {
    // watch receiver for the newest coalesced state
    // broadcast subscription point for lifecycle/progress events
    // owned supervisor task producing the authoritative BatchReport
}
```

The handle exposes snapshot subscription, event subscription, and an async
`wait` method. Dropping a UI receiver does not cancel work. The caller owns the
root `CancellationToken`; cancellation is explicit.

The final `BatchReport` is authoritative and contains one result for every
input in discovery order:

```rust
pub enum FileOutcome {
    Succeeded(JobReport),
    Skipped(SkipReason),
    Planned(DryRunReport),
    Failed(FileFailure),
    Cancelled(CancellationReason),
}
```

`SkipReason` distinguishes `NothingToDo` and `ValidExistingOutput`.
`FileFailure` retains a stable stage (`probe`, `selection`, `plan`, `conflict`,
`execute`, or `internal`) plus a bounded diagnostic. A task panic or unexpected
join failure becomes an `internal` failure; it never panics the batch process.

## Two-phase scheduler

The supervisor performs two bounded phases.

### Phase 1: preparation

Each admitted file receives a child cancellation token and one semaphore
permit. Its task performs:

```text
probe
→ resolve remux selection
→ pure plan
→ inspect existing output
→ ready | skipped | planned | failed
```

Preparation uses a `JoinSet`. The supervisor acquires a permit before spawning,
so thousands of inputs do not become thousands of sleeping Tokio tasks.
Completion order is irrelevant; results are stored by `JobId`.

All preparation completes before execution begins. This deliberate barrier:

- discovers durations before aggregate execution progress starts;
- detects invalid inputs and output conflicts before expensive writes;
- makes dry-run exactly the preparation phase;
- keeps duration-weighted progress monotonic;
- avoids racing two jobs toward the same final destination.

The tradeoff is a visible analysis delay for very large batches. The CLI shows
`Analyzing X/Y`, and preparation itself is parallel and cancellable.

After preparation, the supervisor rejects duplicate destination keys before
execution. Paths use the same resolved-parent rules as the safe M3 transaction.
Every colliding job receives a deterministic conflict failure, rather than
letting one arbitrary winner rewrite a large file. The M3 exclusive atomic
commit remains the final protection against filesystem aliases and external
writers that cannot be proven during preflight.

### Phase 2: execution

Only ready plans enter execution. A fresh `JoinSet` and the same semaphore limit
active complete-file transactions. Each worker calls `Runtime::execute`, which
already owns staging, validation, exclusive atomic publication, process-group
termination, and cleanup.

The scheduler never uses `abort_all` as normal cancellation. It cancels child
tokens, stops admission, drains the `JoinSet`, and returns only after every
started worker has completed cleanup. `kill_on_drop` remains a last-resort panic
safety net in the backend, not the normal control path.

## Failure and cancellation semantics

With the default `Continue` policy, a probe, planning, output, FFmpeg, or
validation failure affects only its file. Other jobs continue, and the final
report contains all outcomes.

With `FailFast`, the first failure:

1. stops admission of new jobs in the current phase;
2. cancels every active child token;
3. drains and reaps every active task;
4. marks never-started and interrupted files as cancelled with reason
   `fail-fast`;
5. preserves the original failure as the cause of the batch failure.

Fail-fast during preparation starts no execution phase. Fail-fast during
execution never removes already committed outputs, but every in-flight staging
transaction is cleaned.

User cancellation is distinct from fail-fast. It cancels both the scheduler and
all children, waits for cleanup, emits one terminal cancelled event, and maps to
exit code 130 even if earlier files failed. A second Ctrl+C keeps the same
orderly path; there is no orphan-producing fast exit.

Batch exit codes retain the M4 contract:

- all success/skip/dry-run: 0;
- multi-file batch with any failure: 1;
- one-file failure: its stage-specific code 3–6;
- user cancellation after cleanup: 130.

Files cancelled only because of fail-fast do not count as additional failures.

## Event and state pipeline

Workers send best-effort raw progress through a bounded `mpsc` channel. A
single aggregator owns mutable progress state, preventing concurrent renderers
or JSON writers from inventing ordering. Lifecycle completion comes from
`JoinSet` results and is never inferred from progress delivery.

The aggregator publishes:

- `watch<Arc<BatchSnapshot>>` for the latest coalesced state;
- `broadcast<BatchEvent>` for UI lifecycle/progress subscribers.

Slow subscribers may lag and miss intermediate broadcast values. They can
recover immediately from the watch snapshot. They cannot lose final truth,
because `BatchHandle::wait` returns the authoritative report.

Events include `batch_started`, `preparation_started`, `file_prepared`,
`execution_started`, `file_progress`, `file_finished`, and exactly one of
`batch_finished` or `batch_cancelled`. Every event includes a stable `JobId`
where applicable. The CLI remains the owner of ADR-0006 JSON DTOs and assigns
one monotonically increasing NDJSON sequence from this single event stream.

Progress channels are bounded from the concurrency limit and capped. Raw
`Advanced` events may be coalesced with `try_send`; lifecycle and final results
are not dropped. Channel closure is normal completion, not success evidence.

## Progress, speed, and ETA

Execution aggregate progress is duration weighted, as accepted in ADR-0004.
For every plan with known duration:

```text
position = clamp(out_time_us, 0, duration_us)
aggregate = sum(position) / sum(duration)
```

Completed successful jobs contribute their full duration. Failed or cancelled
jobs contribute their last clamped position and remain visibly terminal; the
count summary is always authoritative. When any executable plan has unknown
duration, aggregate percentage and ETA are `None` rather than fabricated.
Preparation uses a file-count progress value (`prepared / total`) separately.

Per-file ETA uses remaining media time and FFmpeg's positive `speed_milli` when
available. Aggregate ETA is shown only after at least two advancing samples and
two seconds of monotonic elapsed time. It uses the observed change in weighted
position over wall time, is labelled an estimate, and returns unknown for zero,
stalled, regressing, or indeterminate input. Raw backend values are retained for
diagnostics; only presentation values are clamped.

## CLI rendering and machine output

On an interactive stderr, `indicatif::MultiProgress` renders one aggregate bar
plus at most `max_concurrency` active file bars. Completed file bars are cleared
and concise outcomes are printed without disturbing active bars. Preparation
has a count bar; execution switches to duration progress when determinate.

The progress UI remains disabled for non-TTY stderr, `TERM=dumb`, `--quiet`,
`--json`, and `--json-progress`. Final human and JSON results are ordered by
discovery order, never completion order.

ADR-0006 remains version 1. M5 adds optional fields and event variants:

- `job_id`, `path`, and `stage` on per-file events;
- `active`, `queued`, `completed`, `total`, `progress_milli`, and `eta_ms` on
  aggregate snapshots;
- resolved `jobs`, `storage_profile`, and `failure_policy` on batch start;
- explicit `cancel_reason` for cancelled files.

Existing event names and fields are not removed or reinterpreted. Snapshot and
deserialization tests prove that M4 consumers can ignore the additions.

## Testing strategy

Deterministic mock-backend tests use barriers and controlled channels rather
than wall-clock sleeps wherever possible. They cover:

1. maximum active preparation and execution never exceed the permit count;
2. `jobs = 1` preserves sequential behavior;
3. out-of-order completion still produces discovery-ordered reports;
4. one failed file does not prevent unrelated success under `Continue`;
5. fail-fast stops admission and cancels/reaps active workers;
6. cancellation returns only after every worker acknowledges cancellation;
7. no final output or `.sonicmux-*` staging entry remains after cancellation;
8. duplicate destinations are rejected before execution;
9. dropped/lagged event receivers do not change the final report;
10. duration aggregation, clamping, unknown duration, speed, and ETA rules;
11. strict config parsing and CLI/env/file/default precedence;
12. CLI snapshots for new flags and JSON/NDJSON additions.

A gated real-FFmpeg integration test starts a deliberately long batch, waits
until FFmpeg and staging are observable, cancels it, and verifies:

- the process groups have exited and were reaped;
- no final output was published;
- no private staging directory remains;
- the terminal result is cancellation, not a generic execution failure.

The existing Ubuntu FFmpeg CI job runs this test. Unit tests remain independent
of an installed FFmpeg. Windows and macOS continue to run the mock cancellation
contract and the full workspace suite; platform process-group behavior remains
covered by the M3 backend implementation and release manual checks.

## Benchmark protocol

Performance data is documentation, not a CI gate. A checked-in benchmark helper
prepares four distinct copies of the same representative MKV and invokes:

```text
hyperfine --warmup 1 \
  'sonicmux convert INPUTS --jobs 1 --output-dir OUT-1' \
  'sonicmux convert INPUTS --jobs N --output-dir OUT-N'
```

Each timed run starts with only its validated benchmark output directory
cleaned. Inputs are never modified. The report records SonicMux commit, build
profile, OS, CPU, memory, storage type, filesystem, FFmpeg version, input count,
total bytes, video/audio codecs, duration, output mode, selected `N`, warmup,
run count, mean, standard deviation, range, and relative result.

The README table reports measured values even if parallel execution is slower.
No universal speedup language is allowed. The canonical M5 measurement uses the
available local NVMe/SSD system. If no genuine HDD is available, the HDD row is
explicitly `not measured` rather than fabricated; `--storage-profile hdd`
remains justified by the conservative one-job behavior in ADR-0004.

Private copyrighted media is never committed. The benchmark input may be a
locally held representative file; the report publishes only bounded technical
metadata and a hash. A reproducible generated MKV can supplement it but is not
mislabelled as the real-media measurement.

## Documentation changes

M5 updates:

- `README.md`: parallel examples, scheduler settings, measured benchmark table,
  cancellation semantics, and removal of the sequential limitation;
- `docs/cli.md`: new flags, precedence, progress, fail-fast, and exit behavior;
- the starter TOML emitted by `config init`;
- ADR-0004 only if implementation evidence forces a decision change. No ADR is
  rewritten merely to match code.

## Definition of Done

M5 is complete when:

1. the approved runtime scheduler is used by `sonicmux convert`;
2. concurrency is bounded and final results are deterministic;
3. partial failures continue by default and fail-fast behaves as documented;
4. aggregate progress and ETA follow the determinate/unknown rules;
5. cancellation tests prove staging cleanup and process reaping;
6. a 1-vs-N `hyperfine` result and full methodology are documented without an
   unsupported performance claim;
7. README, CLI docs, config examples, completions, man output, and machine
   protocol fixtures are updated;
8. `cargo fmt --all --check`, workspace Clippy with warnings denied, all tests,
   rustdoc warnings, MSRV check, and the real-FFmpeg CI path are green;
9. M5 is delivered as one reviewable PR with Conventional Commits.

## Deferred work

- TUI rendering and key bindings: M6;
- GUI event bridge and sidecar delivery: M7;
- release artifacts and broad manual platform matrix: M8;
- automatic storage detection, adaptive concurrency, byte-weighted progress,
  pause/resume, persistent queues, and recovery across process restarts: later
  milestones after measurement.
