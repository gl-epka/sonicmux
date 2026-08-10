# M4 design: complete sequential CLI

- Status: Accepted
- Date: 2026-08-11
- Milestone: M4

## Scope

M4 turns the M3 execution core into an installable, scriptable CLI. It delivers
the complete command set accepted at M0:

- `probe`;
- `convert`;
- `scan`;
- `config`;
- `presets`;
- `doctor`;
- `completions`;
- `man`.

It also delivers configuration precedence, sequential multi-file operation,
dry-run, human progress, versioned JSON/NDJSON output, Ctrl+C cancellation,
file logging, documented exit codes, CLI UX tests, a root README, and a verified
`cargo install --path crates/cli --locked` path.

M4 deliberately remains sequential. M5 replaces the sequential admission loop
with the approved bounded scheduler and adds `--jobs`, storage profiles, global
progress/ETA, and batch-wide cancellation policy. M4 reports per-file progress
and a deterministic sequential batch summary, so it is useful before M5 without
putting scheduling policy in the CLI crate.

M4 does not expose `--in-place` or `--overwrite`. Both require a recoverable
replacing transaction that M3 explicitly deferred; accepting a flag that fails
as “not implemented” or silently weakening atomicity would be worse than
omitting it. A valid existing output can be safely recognized and skipped in
M4. Replacing transactions require a focused follow-up design before M5.

Only Matroska `.mkv` inputs are accepted. CLI/TUI still use system FFmpeg and do
not download or bundle it.

## Command surface changes from the M0 draft

The command names, output channels, defaults, and exit-code table in
[`docs/cli.md`](cli.md) remain the contract except for these explicit M4
refinements:

1. `--jobs`, `--storage-profile`, `--continue-on-error`, and `--fail-fast` are
   deferred to M5.
2. `--in-place`, `--backup-suffix`, `--overwrite`, and `--yes` are deferred to a
   separately approved replacing-transaction extension.
3. Human progress is drawn on stderr and is enabled only when stderr is a TTY;
   stdout remains clean result data.
4. `--profile` accepts a built-in name or a configured custom profile name
   rather than being a closed `ValueEnum`.
5. A remux selector is `first-compatible`, a decimal stream index, or a language
   tag. A language selector matching multiple compatible tracks is an error that
   lists the candidate stream indices; SonicMux never guesses among them.
6. M4 processes discovered inputs in stable path order. It continues after a
   per-file failure and returns batch exit code 1 when the batch is mixed.
7. `probe --compact` remains. The M0 draft's `probe --show-raw` is deferred:
   M2 intentionally retains a bounded backend-neutral media model rather than
   raw FFprobe fields, and M4 will not add an FFmpeg-shaped escape hatch to the
   public schema without a separate raw-field contract.
8. Media replacement flags and `config init --overwrite` are both omitted in
   M4. `config init` always uses create-new semantics.
9. `--json` is available for `probe`, `scan`, `convert`, `config`, `presets`,
   and `doctor`. `--json-progress` is available only for the media-operation
   commands `probe`, `scan`, and `convert`. `completions` and `man` emit their
   raw artifacts and reject either machine-result flag.

The implemented `--help` becomes the canonical text after snapshot review, and
`docs/cli.md` is regenerated to match it during M4 implementation.

## Crate boundaries

M4 keeps argv, terminal detection, styling, and JSON presentation in
`sonicmux-cli`. Reusable filesystem/config/application behavior lives in
`sonicmux-runtime`. FFmpeg executable resolution and process capability parsing
live in `sonicmux-ffmpeg`.

```text
sonicmux-cli
├── args.rs          clap-only types and syntactic validation
├── command.rs       command dispatch into Runtime
├── dto.rs           versioned JSON/NDJSON presentation DTOs
├── human.rs         tables, summaries, color, and progress
└── main.rs          composition, tracing guard, Ctrl+C, exit code

sonicmux-runtime
├── application.rs   one-file probe/plan/execute service
├── config.rs        typed partial sources and pure precedence merge
├── discovery.rs     deterministic files/directories/glob expansion
├── execution.rs     existing M3 safe transaction
└── observability.rs console plus optional structured file logging

sonicmux-ffmpeg
├── discovery.rs     explicit path/directory/PATH toolchain resolution
├── capabilities.rs  bounded version and codec/container inspection
└── existing M2/M3 probe and execution modules
```

The CLI composition root may resolve and construct `FfmpegCliBackend`, as
ADR-0005 already permits. Command handlers do not call FFmpeg-specific methods;
they use `sonicmux-runtime::Runtime` and the `MediaBackend` port. This adds a
composition-only `sonicmux-cli -> sonicmux-ffmpeg` dependency and does not move
media protocol logic into the UI.

## Runtime application API

M4 adds a small reusable facade rather than duplicating the probe/plan/execute
sequence in every UI:

```rust
pub struct Runtime {
    backend: Arc<dyn MediaBackend>,
}

pub enum ExistingOutputOutcome {
    Absent,
    Valid,
    Conflict { mismatches: Vec<ValidationMismatch> },
}

impl Runtime {
    pub async fn probe(
        &self,
        input: &Path,
        cancel: CancellationToken,
    ) -> Result<MediaInfo, RuntimeError>;

    pub fn plan(
        &self,
        media: &MediaInfo,
        policy: &PlanningPolicy,
    ) -> Result<PlanOutcome, RuntimeError>;

    pub async fn inspect_existing_output(
        &self,
        plan: &JobPlan,
        cancel: CancellationToken,
    ) -> Result<ExistingOutputOutcome, RuntimeError>;

    pub async fn execute(
        &self,
        plan: Arc<JobPlan>,
        progress: mpsc::Sender<ProgressEvent>,
        cancel: CancellationToken,
    ) -> Result<JobReport, RuntimeError>;

    pub async fn doctor(
        &self,
        request: CapabilityRequest,
        cancel: CancellationToken,
    ) -> Result<BackendCapabilities, RuntimeError>;
}
```

The exact field visibility may be constructors/getters rather than public
fields. The contract is that the facade accepts typed domain values, owns no
argv or output formatting, and can be instantiated with a mock backend.

`inspect_existing_output` performs no mutation. It rejects a symlink or
non-regular final path, probes a regular output, and calls M3's pure validation.
`Valid` becomes a successful skip. `Conflict` is an execution-category failure;
M4 never deletes or replaces the conflicting file. A destination appearing
after inspection is still caught by M3's atomic exclusive commit.

## Backend capability port

`doctor` must not make a UI depend on FFmpeg output spelling. M4 therefore adds
one neutral method to `MediaBackend`:

```rust
pub enum MediaCapability {
    Demuxer(String),
    Muxer(String),
    Decoder(String),
    Encoder(String),
}

pub struct CapabilityRequest {
    required: Vec<MediaCapability>,
}

pub struct CapabilityCheck {
    capability: MediaCapability,
    available: bool,
    detail: Option<String>,
}

pub struct BackendToolInfo {
    role: BackendToolRole,
    path: PathBuf,
    version: Option<String>,
}

pub struct BackendCapabilities {
    backend_name: String,
    tools: Vec<BackendToolInfo>,
    checks: Vec<CapabilityCheck>,
    warnings: Vec<String>,
}

#[async_trait]
pub trait MediaBackend: Send + Sync {
    // Existing probe and execute methods.
    async fn capabilities(
        &self,
        request: CapabilityRequest,
        cancel: CancellationToken,
    ) -> Result<BackendCapabilities, BackendError>;
}
```

The request for the default CLI checks Matroska demux/mux, DTS and TrueHD
decoders, and the selected target encoder. FFmpeg capability commands use direct
argument arrays and owned process groups. Version/list stdout and stderr are
read concurrently with explicit limits; cancellation kills and reaps exactly as
probe and execute do.

The adapter uses FFmpeg's documented `-version`, `-decoders`, `-encoders`, and
`-formats` generic options. Parsers receive checked-in text fixtures for old and
current FFmpeg layouts. Unknown list lines are ignored. Oversized output, launch
failure, and non-zero exit remain typed backend errors. Missing required
capabilities are typed negative checks in a successfully returned report, so
`doctor` can display all missing items at once before returning exit code 3.

Tool versions are diagnostic strings, not semver. `doctor` warns when ffmpeg and
ffprobe resolve from different parent directories or report different leading
version numbers. It does not reject distribution-specific suffixes.

## FFmpeg toolchain discovery

Resolution follows ADR-0002:

```text
explicit --ffmpeg-path
> SONICMUX_FFMPEG_PATH
> config ffmpeg.path
> bundled sidecar (none in CLI/TUI M4)
> PATH
```

An explicit value may be:

- a directory containing the platform's `ffmpeg[.exe]` and `ffprobe[.exe]`;
- an exact `ffmpeg[.exe]` path, in which case the sibling ffprobe name is used.

Other filenames are rejected rather than guessed. PATH lookup uses `which` so
Windows `PATHEXT` semantics are retained. Discovery returns the named
`FfmpegToolchainPaths` pair but does not execute it. `doctor` can therefore
separate “not found”, “could not launch”, “wrong origin”, and “missing codec”.

Normal media commands fail before probing with exit code 3 if the pair cannot be
resolved. `doctor` renders the resolution failure as its primary check and also
returns 3.

## Configuration model

M4 implements the accepted precedence:

```text
CLI > SONICMUX_* environment > selected TOML > defaults
```

Each source first becomes a typed partial override. Merging is a pure function
that retains provenance:

```rust
pub enum ConfigSource { Cli, Environment, File, Default }

pub struct Sourced<T> {
    value: T,
    source: ConfigSource,
}

pub struct PartialConfig { /* every configurable field is Option<T> */ }
pub struct EffectiveConfig { /* validated Sourced<T> fields */ }

pub fn merge_config(
    defaults: DefaultConfig,
    file: PartialConfig,
    environment: PartialConfig,
    cli: PartialConfig,
) -> Result<EffectiveConfig, ConfigError>;
```

TOML parsing and file/environment access stay in runtime; the final
`PlanningPolicy` remains a pure core value. Configuration structs use
`serde(deny_unknown_fields)` at every table so misspelled keys fail.

The platform default is `ProjectDirs::from("", "", "sonicmux")/config.toml`.
An absent default file is normal. An explicit `--config` path that is absent,
unreadable, oversized, or not a regular file is an error. Config input is capped
at 1 MiB before TOML parsing.

The initial file shape is versioned:

```toml
version = 1
profile = "generic-tv"

[audio]
codec = "ac3"
bitrate = "640k"
channels = "keep-up-to-5.1"
mode = "add"

[ffmpeg]
path = "/optional/path/or/directory"

[output]
directory = "/optional/output/directory"

[profiles.living-room]
unknown-codec = "reject"

[profiles.living-room.codecs.ac3]
maximum-channels = 6

[profiles.living-room.codecs.aac]
maximum-channels = 2
allowed-layouts = ["mono", "stereo"]
```

Configured profile codec keys are the stable families `ac3`, `eac3`, `aac`,
`mp3`, `dts`, `truehd`, `flac`, `opus`, `vorbis`, and `pcm`. Unknown family names,
zero channels, empty layouts, unsupported config versions, configured names that
collide with built-in profile names, or selecting `custom` without
`[profiles.custom]` fail with exit code 2. All configured profile names are
unique by TOML construction; names are compared exactly after parsing.

Environment parsing uses `var_os` for paths and `var` for textual typed values.
The supported variables are documented and finite:

```text
SONICMUX_CONFIG
SONICMUX_FFMPEG_PATH
SONICMUX_PROFILE
SONICMUX_CODEC
SONICMUX_BITRATE
SONICMUX_CHANNELS
SONICMUX_MODE
SONICMUX_OUTPUT_DIR
SONICMUX_COLOR
SONICMUX_LOG_FILE
```

`--config` wins over `SONICMUX_CONFIG`. `config show --sources` prints the
provenance retained by `Sourced<T>`. `config init` creates the parent directory,
uses create-new semantics, syncs the file, and refuses replacement unless a
future explicit overwrite transaction is designed; M4 intentionally has no
`config init --overwrite`.

## Input discovery

Discovery is synchronous filesystem work executed through `spawn_blocking` by
the runtime facade. Its typed API accepts literal `OsString` inputs separately
from Unicode glob patterns so non-UTF-8 literal paths remain supported.

Rules:

- explicit regular `.mkv` files are included;
- explicit symlinks are rejected unless `--follow-links` is present;
- a directory includes direct children, and `--recursive` enables deeper
  traversal;
- links encountered during traversal are not followed by default;
- loops and permission errors are reported per root and never silently ignored;
- include/exclude globs match paths relative to each discovery root;
- only case-insensitive `.mkv` extensions are admitted;
- results are lexically sorted and de-duplicated before any probe starts;
- an unmatched explicit glob is an input-discovery error, not an empty success.

`glob` expands explicit shell-independent patterns and `walkdir` traverses
directories. `globset` 0.4.19 applies include/exclude sets; 0.4.20 is not selected
because it requires Rust 1.88 and the workspace MSRV is 1.85.

`probe` accepts literal files only in M4. `scan` and `convert` use discovery.
This prevents an accidental expensive directory walk when a user only wants to
inspect one exact file.

## Planning and sequential batch flow

For each discovered file, M4 runs:

```text
resolve effective config
→ resolve output path
→ probe input
→ resolve remux language selector, if any
→ pure plan
→ render plan when dry-run
→ inspect existing output
→ valid: skip
→ absent: execute through M3 transaction
→ conflict: fail without mutation
```

Default naming remains `<stem>.sonicmux.mkv`. `--output PATH` requires one
literal input file. `--output-dir DIR` must already exist in M4 and applies to
every discovered input. M4 does not create output directories as a side effect
of dry-run or conversion.

Sequential batch operation continues after a per-file probe, plan, or execution
failure. This is a simple ordered loop in runtime, not the M5 scheduler. Result
ordering always matches discovery order, including JSON output.

`--dry-run` performs discovery, tool resolution, probe, remux-selector
resolution, planning, and existing-output inspection. It never calls execute,
creates a staging path, creates an output directory, or writes configuration.

## Remux selector

`--default-audio` parsing is syntactic in CLI and semantic after probe:

```rust
pub enum AudioSelectionRequest {
    FirstCompatible,
    StreamIndex(StreamIndex),
    Language(String),
}
```

The resolver first filters to compatible audio under the selected profile.
Index must identify one compatible stream. Language comparison is ASCII
case-insensitive against retained tags. Zero matches returns the existing typed
remux selection error; multiple matches returns a new ambiguity error containing
only bounded stream indices and titles. The resolved planner input remains
`AudioSelector::StreamIndex`, so language never leaks into the pure plan.

## Human output and progress

Human result data goes to stdout. Diagnostics, warnings, logs, and progress go
to stderr. `anstream` implements `auto|always|never`, terminal adaptation on
Windows, and `NO_COLOR`; an explicit `--color` wins over the environment.

`indicatif` renders one M4 file bar on stderr. It is hidden when stderr is not a
terminal, `TERM=dumb`, `--quiet`, `--json`, or `--json-progress` is active.
Duration comes from `JobPlan`; `out_time_us` is clamped only for presentation.
Unknown duration uses a spinner. Raw progress remains unchanged in backend
reports.

The CLI receives best-effort backend events while awaiting the authoritative
execution future. A dropped intermediate event cannot turn success into failure
or vice versa. The final report always produces the terminal human/JSON result.

Ctrl+C cancels a root `CancellationToken`. The currently running backend kills
and reaps its process group through M3, then runtime cleans staging. M4 returns
130 only after cleanup. A second Ctrl+C is not given a destructive fast path;
the same orderly cancellation remains in force.

## JSON and NDJSON protocol

ADR-0006 defines the machine protocol. `--json` writes exactly one final JSON
document followed by one newline. `--json-progress` writes one JSON object per
line: batch start, file start, progress snapshots, file terminal events, and one
batch terminal event. No human text, tracing, or ANSI reaches stdout.

`--json` applies to `probe`, `scan`, `convert`, `config`, `presets`, and
`doctor`. NDJSON progress applies only to `probe`, `scan`, and `convert`, where
there is an operation lifecycle. `completions` and `man` write unwrapped shell
or ROFF artifacts; clap rejects their use with either machine-result flag.

CLI-owned DTOs convert domain values explicitly. Core types do not gain Serde
derives, avoiding accidental public-schema changes when internal fields evolve.
Every envelope contains:

```json
{
  "schema": "sonicmux.result",
  "version": 1,
  "command": "convert",
  "status": "success",
  "files": []
}
```

NDJSON additionally contains a monotonically increasing `sequence` number and
an `event` discriminator. Integer units are explicit (`bitrate_bps`,
`duration_us`, `speed_milli`). Unknown/future enum values serialize as stable
`unknown` objects rather than debug strings.

Paths never make serialization fail. A path DTO contains a lossy display string
plus an exact, round-trippable platform representation: UTF-8 bytes when exact,
arbitrary Unix bytes as hex, or Windows WTF-16 code units encoded little-endian
as hex. Consumers can use the display field and retain the native value when
exact round-tripping matters.

One JSON/NDJSON object is serialized into a bounded in-memory buffer before one
`write_all`, preventing partial lines from interleaving. Media metadata remains
bounded by M2/M3 validation. A broken stdout pipe ends read-only commands
successfully; during conversion it cancels current work and returns an output
failure after cleanup because the controlling consumer disappeared.

JSON fixture tests treat additive fields as compatible within schema version 1,
but removal, rename, unit change, or semantic reinterpretation requires version
2.

## Logging

M4 refines observability initialization:

```rust
pub struct ObservabilityOptions {
    pub filter: String,
    pub console: bool,
    pub file: Option<PathBuf>,
}

pub struct ObservabilityGuard { /* retains WorkerGuard */ }

pub fn init_tracing(
    options: ObservabilityOptions,
) -> Result<ObservabilityGuard, ObservabilityError>;
```

Console logs are human-readable stderr. `--log-file` writes newline-delimited
structured JSON through `tracing-appender`'s bounded non-blocking writer. The
guard lives until `main` returns so normal errors and cancellation flush logs.
M4 uses one explicit file and no implicit rotation; automatic rotating default
logs can be designed when GUI support needs them.

`RUST_LOG` remains the filter source unless verbosity flags explicitly refine
the SonicMux target level. `-v` increases only SonicMux verbosity; `-q` hides
non-error human output but does not discard file diagnostics.

## CLI error and exit behavior

`main` returns `std::process::ExitCode`; it does not call `process::exit`, so
temporary guards and the log worker are dropped normally.

`CliFailure` carries a typed phase, optional path, bounded message, and the
documented exit code. Human mode renders one concise error plus a `-v` hint.
Machine modes serialize the same category without a color-eyre debug dump.
Unexpected internal errors retain a source chain in logs.

The accepted exit table remains:

| Code | Meaning |
| ---: | --- |
| 0 | all work succeeded or validly skipped |
| 1 | sequential batch completed with one or more file failures |
| 2 | invalid argv or configuration |
| 3 | FFmpeg/FFprobe discovery or capability failure |
| 4 | discovery or probe failure |
| 5 | planning failure |
| 6 | execution, validation, output conflict, or safe commit failure |
| 130 | user cancellation after cleanup |

Clap help and version return 0; Clap usage errors return 2. For a one-file
command, the phase-specific code is returned. For a mixed multi-file command,
1 takes precedence and every exact file category remains in the report.

## Completions and man page

The derived `Cli` implements `CommandFactory`. `clap_complete::aot::generate`
writes Bash, Elvish, Fish, PowerShell, or Zsh completions to stdout.
`clap_mangen::Man::render` writes section-1 ROFF to stdout or a create-new file.

Both commands operate before config/tool discovery and never require FFmpeg.
Their snapshot tests prove stdout contains only the generated artifact. Release
packaging can invoke the same commands rather than maintaining checked-in copies.

## Dependency set

Versions checked on 2026-08-11 and compatible with the workspace MSRV unless
noted:

- `clap 4.6.6` with `derive` and `cargo`;
- `clap_complete 4.6.9` using stable AOT generation only;
- `clap_mangen 0.3.2`;
- `indicatif 0.18.6` with only the required Unicode-width feature;
- `anstream 1.0.0`;
- `directories 6.0.0`;
- `which 8.0.5`;
- `glob 0.3.4`;
- `walkdir 2.5.0`;
- exact `globset =0.4.19` because 0.4.20 requires Rust 1.88;
- `toml 1.1.4`;
- `tracing-appender 0.2.5`;
- `trycmd 1.2.1` and `assert_cmd 2.2.2` as CLI dev dependencies.

The implementation lockfile and `cargo +1.85.0 check --workspace --all-targets`
remain authoritative. A dependency that raises transitive MSRV or introduces an
incompatible license is rejected rather than weakening the workspace policy.

Relevant APIs were checked against the current
[`clap` documentation](https://docs.rs/clap/4.6.6/clap/),
[`clap_complete` AOT documentation](https://docs.rs/clap_complete/4.6.9/clap_complete/aot/),
[`clap_mangen` documentation](https://docs.rs/clap_mangen/0.3.2/clap_mangen/struct.Man.html),
[`indicatif` documentation](https://docs.rs/indicatif/0.18.6/indicatif/),
[`trycmd` documentation](https://docs.rs/trycmd/1.2.1/trycmd/),
[`directories` documentation](https://docs.rs/directories/6.0.0/directories/),
[`which` documentation](https://docs.rs/which/8.0.5/which/),
[`toml` documentation](https://docs.rs/toml/1.1.4/toml/),
[`tracing-appender` documentation](https://docs.rs/tracing-appender/0.2.5/tracing_appender/),
the [FFmpeg generic-option manual](https://ffmpeg.org/ffmpeg.html), and the
[FFprobe version-output manual](https://ffmpeg.org/ffprobe-all.html).

## Tests

### Pure/unit tests

- bitrate, profile, remux-selector, and output-path conversion;
- four-source config precedence with provenance;
- unknown keys, unsupported versions, invalid codec rules, and 1 MiB cap;
- default config location and explicit-path semantics;
- deterministic discovery ordering, include/exclude, recursive depth, symlink
  rejection/following, loops, unmatched globs, and non-UTF-8 literal paths where
  supported;
- toolchain resolution for directory, executable, PATH, missing sibling, and
  Windows executable suffix;
- bounded capability parsing across fixtures;
- domain-to-human and domain-to-JSON DTO conversion;
- exact exit-code mapping.

### CLI UX tests

`trycmd` snapshots cover top-level and every subcommand help, invalid conflicts,
bad enum/bitrate/profile values, and stable exit 2 output. `assert_cmd` covers:

- stdout/stderr separation;
- `NO_COLOR` and explicit color precedence;
- completions and man output;
- config path/show/init/validate in temporary directories;
- JSON schema fixtures and one-newline termination;
- non-TTY progress suppression;
- exact exit codes;
- input paths containing spaces and non-UTF-8 bytes where supported.

Runtime facade tests inject `MockBackend` for probe, plan, dry-run, existing valid
skip, conflict, execution, progress, failure, and cancellation. They assert that
dry-run performs no writes and cancellation returns only after backend reaping.

### Real-media test

The existing generated DTS fixture gains a gated CLI path that runs `probe`,
`convert --json`, and output `probe`. It verifies JSON parsing, output track
order, metadata, chapters, no staging residue, and successful installation-path
composition. CI keeps one Ubuntu real-FFmpeg job; the normal three-platform UX
suite uses mocks and text fixtures.

## README and installation

M4 adds a root `README.md` with the DTS problem/solution, FFmpeg prerequisite,
installation from source, quick-start commands, command links, safety model,
platform support, current limitations, license, and acknowledgements. Demo media,
benchmarks, release downloads, package-manager recipes, TUI, and GUI sections are
clearly marked as later milestones rather than fabricated.

The DoD installation check uses a temporary isolated root:

```text
cargo install --path crates/cli --locked --root <temporary-directory>
<temporary-directory>/bin/sonicmux --version
<temporary-directory>/bin/sonicmux doctor
```

The first two commands must work without FFmpeg; `doctor` must return either a
successful report or documented code 3 rather than crash.

## Definition of Done

M4 is complete when:

- this design and ADR-0006 are accepted;
- all eight command groups have reviewed help snapshots;
- probe, sequential convert/scan, config/presets, and doctor work through shared
  runtime/backend boundaries;
- dry-run makes no filesystem mutation;
- human, JSON, and NDJSON output obey stdout/stderr and versioning contracts;
- progress hides on non-TTY and Ctrl+C cleans/reaps before exit 130;
- valid existing outputs skip and conflicting outputs remain untouched;
- completions and a man page generate without FFmpeg;
- `trycmd`/`assert_cmd`, mock runtime, and gated real-media tests pass;
- the README contains honest installation and quick-start examples;
- `cargo install --path crates/cli --locked` works from a clean temporary root;
- `cargo fmt --all --check` passes;
- `cargo clippy --workspace --all-targets --all-features -- -D warnings` passes;
- `cargo test --workspace --all-targets` passes;
- `RUSTDOCFLAGS='-D warnings' cargo doc --workspace --all-features --no-deps`
  passes;
- `cargo +1.85.0 check --workspace --all-targets` passes;
- CI is green on Linux, Windows, and macOS.

## Approval points

Approval of M4 accepts these refinements:

1. M4 is a complete sequential CLI; parallel scheduler flags remain M5.
2. M4 omits overwrite/in-place instead of exposing unsafe or nonfunctional
   flags, including `config init --overwrite`; their recoverable transaction
   receives a separate design.
3. valid existing output is probed, validated, and skipped without mutation;
   conflicting output fails with code 6.
4. runtime gains a reusable `Runtime` application facade and backend gains a
   neutral capability-inspection method for `doctor`.
5. CLI composition may depend on `sonicmux-ffmpeg` only to resolve and construct
   the concrete adapter; command logic stays backend-neutral.
6. configuration is versioned strict TOML, uses platform directories, and keeps
   typed source provenance under CLI > env > file > defaults.
7. custom profile names are open text resolved from config, may not shadow
   built-ins, and `custom` requires `[profiles.custom]`; ambiguous language
   remux selectors fail rather than choosing silently.
8. discovery is sorted, de-duplicated, non-following by default, and unmatched
   explicit globs are errors.
9. stable machine output uses CLI-owned versioned DTOs, exact-unit fields, and
   non-UTF-8-safe path objects as defined by ADR-0006.
10. progress and diagnostics use stderr; stdout is exclusively result/artifact
    data.
11. `main` returns typed exit codes without `process::exit`, preserving cleanup
    and log flushing.
12. `globset` is pinned to 0.4.19 until the workspace MSRV moves past 1.85.
13. `probe --show-raw` is deferred until raw backend fields have a bounded,
    backend-neutral public contract; `probe --compact` remains in M4.
14. final JSON is supported by result-producing commands, while NDJSON progress
    is limited to media operations and artifact generators remain unwrapped.
