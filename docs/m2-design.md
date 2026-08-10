# M2 design: probe, compatibility policy, and pure planning

- Status: Proposed
- Date: 2026-08-10
- Milestone: M2

## Scope

M2 implements four connected pieces:

1. invoke `ffprobe` and parse its JSON output;
2. convert adapter-specific JSON into a validated domain model;
3. classify audio with built-in or custom compatibility policies;
4. build a deterministic `JobPlan` without filesystem or process access.

M2 does not execute FFmpeg conversions, create output files, schedule batches,
render final CLI commands, or implement TUI/GUI behavior. Those boundaries belong
to later milestones.

## FFprobe protocol

The external adapter invokes this argument array directly, without a shell:

```text
ffprobe
-v
error
-of
json
-show_streams
-show_format
-show_chapters
ABSOLUTE_INPUT_PATH
```

The exact executable comes from the already approved discovery policy. The input
is resolved to an absolute path before argument construction, avoiding ambiguity
for names beginning with `-` without relying on undocumented `--` behavior.

The original requirement used `-print_format json`. Local FFprobe 8.1.1 marks
`-print_format` as deprecated, while the current FFprobe documentation defines
`-of` as an alias of `-output_format`. M2 therefore uses `-of json` and updates
ADR-0001 after this design is approved.

Standard output is exclusively JSON. Standard error is captured separately and
reported as a bounded diagnostic tail. The adapter rejects oversized probe
output rather than buffering without limit; the initial hard limit is 64 MiB.
The process exit status is checked before JSON is trusted.

FFprobe legitimately omits inapplicable JSON fields. It can also encode numeric
values as strings and use values such as `N/A`. The parser must accept missing or
unknown optional values while rejecting invalid required identities such as a
missing stream index or stream type.

### Adapter boundary

Private Serde DTOs mirror FFprobe's unstable external representation:

```rust
struct ProbeDocument {
    streams: Vec<RawStream>,
    chapters: Vec<RawChapter>,
    format: Option<RawFormat>,
}
```

All FFprobe field names and stringly typed numbers stop at these private DTOs.
Conversion to `MediaInfo` is fallible and adds field context to errors. Unknown
JSON fields are ignored so a newer FFprobe remains forward-compatible. Unknown
codec, profile, disposition, and metadata values are retained in the domain
model where doing so is safe.

`sonicmux-ffmpeg` initially exposes an inherent async probe API:

```rust
impl FfmpegCliBackend {
    pub async fn probe(&self, path: &Path) -> Result<MediaInfo, ProbeError>;
}
```

The complete `MediaBackend::execute` interface is deferred to M3, when its
execution and cancellation semantics can be designed and implemented together.
M2 does not add a placeholder method that can only return “not implemented”.

## Domain model

The domain representation lives in `sonicmux-core::model` and has no Serde JSON
shape coupled to FFprobe.

### Primitive types

```rust
pub struct StreamIndex(u32);
pub struct Bitrate(u64);
pub struct ChannelCount(NonZeroU16);
pub struct SampleRate(NonZeroU32);
pub struct Language(String);
pub struct TimeBase { numerator: NonZeroU32, denominator: NonZeroU32 }
pub struct MediaTimestamp { ticks: i64, time_base: TimeBase }
```

`Bitrate` changes the M0 sketch from `u32` to `u64`: container and video bitrates
can exceed `u32`, and the type represents all probed streams rather than only the
AC-3 target. This is an amendment to `docs/architecture.md` on M2 approval.

Time values prefer integer ticks plus FFprobe's rational time base. Decimal
seconds are a validated fallback for format-level duration only. No planning
decision uses `f32` or `f64`, keeping equality and snapshots deterministic.

`Language` retains the trimmed source value rather than pretending every
Matroska language tag is valid BCP 47. Missing language is `None`; it is not
silently rewritten to `und`. Normalized matching can be added as a separate
operation without losing the original tag.

### Media and streams

```rust
pub struct MediaInfo {
    pub path: PathBuf,
    pub format: FormatInfo,
    pub streams: Vec<StreamInfo>,
    pub chapters: Vec<Chapter>,
    pub warnings: Vec<ProbeWarning>,
}

pub enum StreamInfo {
    Video(VideoStream),
    Audio(AudioStream),
    Subtitle(SubtitleStream),
    Attachment(AttachmentStream),
    Data(DataStream),
    Unknown(UnknownStream),
}

pub struct StreamCommon {
    pub index: StreamIndex,
    pub codec_name: String,
    pub codec_profile: Option<String>,
    pub bitrate: Option<Bitrate>,
    pub timing: StreamTiming,
    pub metadata: Metadata,
    pub dispositions: Dispositions,
}
```

Every stream variant contains `StreamCommon`. `Metadata` is a deterministic
`BTreeMap<String, String>` with typed accessors for language and title. Original
key/value pairs remain available for later remux mapping.

Known FFprobe disposition flags are typed booleans. Future flags are retained in
an ordered map. A non-zero FFprobe integer means true. Missing flags mean false.

Attachments retain filename and MIME type when present. Chapters retain their
integer boundaries, time base, and tags. Format information includes detected
format names, duration when known, start time, aggregate bitrate, and global
tags.

Duplicate stream indices, an empty stream list, invalid time-base denominators,
zero channel counts, and structurally impossible audio records are conversion
errors. Missing optional bitrate, language, title, duration, or channel layout
produces `None` or a warning, not a panic.

### Audio codecs and layouts

```rust
pub enum AudioCodec {
    Ac3,
    Eac3,
    Aac,
    Mp3,
    Dts(DtsProfile),
    TrueHd,
    Flac,
    Opus,
    Vorbis,
    Pcm(PcmKind),
    Other(String),
}

pub enum DtsProfile {
    Core,
    HdHighResolution,
    HdMasterAudio,
    Express,
    Unknown(String),
}

pub struct Channels {
    pub count: ChannelCount,
    pub layout_name: Option<String>,
}
```

FFprobe normally reports DTS variants with codec name `dts` and a separate
profile. Codec classification therefore uses both fields. Unrecognized DTS
profiles remain DTS and are incompatible under the generic policy.

Channel layout text is retained because FFmpeg exposes more layouts than the
planner needs. Planning uses the validated count and only interprets names for
explicitly supported downmix/layout decisions.

Public enums that can grow are `#[non_exhaustive]`. Constructors validate
invariants; tuple fields are private. Domain types implement `Display` where UI
or diagnostics need a stable human representation.

## Compatibility policy

Compatibility is data-driven and codec-specific:

```rust
pub struct CompatibilityPolicy {
    pub name: ProfileName,
    rules: BTreeMap<AudioCodecFamily, CodecRule>,
    unknown_codec: UnknownCodecBehavior,
}

pub struct CodecRule {
    pub maximum_channels: Option<ChannelCount>,
    pub allowed_layouts: Option<BTreeSet<String>>,
}

pub enum Compatibility {
    Compatible,
    Incompatible(Incompatibilities),
}

pub struct Incompatibilities {
    primary: IncompatibilityReason,
    additional: Vec<IncompatibilityReason>,
}
```

Reasons are typed and stable: unsupported codec, too many channels, unsupported
layout, unknown codec, and missing required stream facts. The list ordering is
deterministic for snapshots and JSON output.

The built-in `generic-tv` baseline follows ADR-0003: AC-3, AAC, and MP3 are
compatible. DTS of every profile and TrueHD are incompatible. Other known audio
codecs are incompatible and can be converted to the configured target. Completely
unknown codecs are rejected unless a custom policy explicitly enables
`transcode-with-fallback`.

There is no honest universal “Samsung codec list” or “LG codec list” across all
models and firmware. In M2, `samsung`, `lg`, and `dlna` are named conservative
presets inheriting `generic-tv`, with an explicit description that they are
baselines rather than model guarantees. We will not invent unsupported
differences merely to make the profiles look distinct. Model-specific sourced
rules can be added later without changing the planner.

`custom` starts from a named preset and applies typed rule overrides. Invalid
rules such as zero maximum channels or an empty codec identifier fail config
validation. TOML loading and source precedence remain runtime work; M2 tests
policy construction and overrides directly.

## Planning inputs

The approved pure boundary remains:

```rust
pub fn build(media: &MediaInfo, policy: &PlanningPolicy)
    -> Result<JobPlan, PlanError>;
```

`PlanningPolicy` contains all decisions that would otherwise require external
state:

```rust
pub struct PlanningPolicy {
    pub compatibility: CompatibilityPolicy,
    pub target: AudioTarget,
    pub output_mode: OutputMode,
    pub action: RequestedAction,
    pub output_path: PathBuf,
}

pub enum RequestedAction {
    Convert,
    RemuxOnly { selection: AudioSelector },
}
```

The input path comes from `MediaInfo`; the output path is already resolved by the
caller. The planner compares paths lexically to prevent an accidental direct
overwrite, but it never canonicalizes, creates, or probes them.

Target types make invalid combinations difficult to express:

```rust
pub enum AudioTarget {
    Ac3 { bitrate: Ac3Bitrate, layout: TargetLayout },
    Eac3 { bitrate: Eac3Bitrate, layout: TargetLayout },
    Aac { bitrate: AacBitrate, layout: TargetLayout },
}

pub enum TargetLayout {
    KeepUpTo51,
    Stereo,
    Surround51,
}
```

Codec-specific bitrate constructors enforce supported ranges. The exact allowed
FFmpeg encoder values will be checked against the selected encoder before M2
implementation; unsupported values fail before a plan is built rather than being
silently rounded.

## Job plan

```rust
pub struct JobPlan {
    input: PathBuf,
    output: PathBuf,
    action: JobAction,
    streams: Vec<OutputStreamPlan>,
    copy_chapters: bool,
    copy_global_metadata: bool,
    warnings: Vec<PlanWarning>,
    expected: OutputExpectations,
}

pub enum OutputStreamPlan {
    Copy {
        source: StreamIndex,
        dispositions: DispositionPlan,
    },
    EncodeAudio {
        source: StreamIndex,
        target: AudioTarget,
        metadata: MetadataPlan,
        dispositions: DispositionPlan,
    },
}
```

Plans contain argument-neutral operations, never preformatted FFmpeg flags or a
shell command. Every operation points to one exact source index. Output
expectations describe codecs, stream origin, metadata, dispositions, chapters,
and attachments for M3 validation.

Unless a sketch explicitly says otherwise, domain struct fields are private and
exposed through invariant-preserving constructors and read-only accessors. A
planned skip is represented separately as `PlanOutcome::Skip(SkipReason)`, while
an executable plan is `PlanOutcome::Execute(JobPlan)`; a skip therefore cannot
accidentally carry writable output operations.

### Convert algorithm

1. Validate Matroska format identity, unique source indices, at least one audio
   stream, distinct lexical input/output paths, and target invariants.
2. Classify every audio stream exactly once and retain all incompatibility
   reasons.
3. Apply the approved output-mode semantics:
   - `add`: copy all source streams in source order, then append one derivative
     for each incompatible audio stream in source-audio order;
   - `replace`: walk source order, copying compatible/non-audio streams and
     placing a derivative at each incompatible audio stream's position;
   - `only-new`: walk source order, preserve non-audio streams, omit compatible
     audio, and place derivatives at incompatible audio positions.
4. Apply metadata and disposition transformations explicitly.
5. Return `PlanOutcome::Skip(NothingToDo)` for `add` or `replace` when all audio is
   compatible. Return `PlanError::NothingToDo` for `only-new`, because producing
   the requested output would remove every audio stream.
6. Derive validation expectations from the operations, not from a second copy of
   the planning rules.

In `add`, if an incompatible source was default, its derivative gains `default`
and that copied source loses only `default`. If no source audio has `default`,
the first derivative gains it. An already compatible default prevents the
fallback rule, as approved in M0. All other known and unknown dispositions are
preserved.

A derivative copies the source language and metadata. Its title is
`<source title> [AC-3 5.1]` or `AC-3 5.1` when the source has no title. Suffix
formatting is a tested domain function and never depends on locale or FFmpeg
version.

### Remux-only algorithm

1. Classify existing audio and collect compatible candidates.
2. Resolve `AudioSelector::StreamIndex` exactly or choose the first compatible
   stream in source order for `FirstCompatible`.
3. Reject a missing or incompatible explicit selection.
4. Copy all source streams in source order, clear only the `default` flag from
   every other audio stream, and set it on the selected compatible stream.
5. Produce no `EncodeAudio` operation.

Language-based selection remains a CLI concern that resolves to an exact stream
index before calling the planner. This avoids ambiguous planner behavior when a
file contains multiple tracks with the same language.

## Determinism and diagnostics

All maps and sets that affect output use ordered collections. Plans preserve
source ordering where specified. Error and warning ordering is stable. Paths are
redacted to `<INPUT>` and `<OUTPUT>` in snapshots so tests are platform-neutral.

Planner errors are typed:

- unsupported container;
- no audio streams;
- duplicate stream index;
- invalid target;
- input equals output;
- unknown source codec without fallback;
- no compatible remux candidate;
- invalid remux selection;
- nothing to do for `only-new`.

Probe errors distinguish executable launch, non-zero exit, output limit,
malformed JSON, and JSON-to-domain conversion. Diagnostics include the failing
field and stream index when known, but never dump unbounded JSON or all media
metadata.

## Test plan

All policy and planner tests construct `MediaInfo` in memory. FFprobe parser
fixtures use `include_str!`/`include_bytes!`, so tests perform no runtime file or
process access.

### Policy unit tests (minimum 12)

1. generic AC-3 is compatible;
2. generic AAC is compatible;
3. generic MP3 is compatible;
4. DTS core is incompatible;
5. DTS-HD MA is incompatible;
6. unknown DTS profile stays incompatible;
7. TrueHD is incompatible;
8. known unsupported FLAC can target fallback conversion;
9. completely unknown codec is rejected by the built-in policy;
10. custom policy can allow an unknown codec fallback;
11. maximum-channel violation reports a typed reason;
12. incompatibility reason ordering is stable;
13. custom codec override replaces the inherited rule;
14. vendor baseline identifies itself as conservative.

### Planner unit tests (minimum 20)

1. add appends one derivative while copying the source;
2. add handles multiple incompatible tracks in source-audio order;
3. add preserves compatible audio;
4. add transfers default from an incompatible source;
5. add chooses the first derivative when no source is default;
6. add preserves an existing compatible default;
7. replace substitutes an incompatible track in position;
8. replace copies mixed compatible audio;
9. only-new omits every original audio track;
10. only-new errors when no derivative exists;
11. remux chooses the first compatible stream;
12. remux honors an exact compatible stream index;
13. remux rejects an incompatible selection;
14. remux rejects a missing selection;
15. remux emits no encoder operation;
16. video is always copied;
17. subtitles, attachments, and data are copied;
18. chapters and global metadata are marked for copy;
19. language and title metadata are inherited;
20. title suffix without a source title is deterministic;
21. no-audio input is rejected;
22. non-Matroska input is rejected;
23. equal input/output paths are rejected;
24. duplicate indices are rejected;
25. unknown codec without fallback is rejected;
26. equal inputs produce equal plans.

### Parser and property tests

Checked-in JSON fixtures cover DTS core, DTS-HD MA, TrueHD, mixed compatible and
incompatible audio, chapters, attachments, missing optional fields, unknown
fields, numeric `N/A`, duplicate indices, and malformed JSON.

Property tests cover:

- no source index appears twice unless `add` intentionally creates one copy and
  one derivative;
- `replace` never changes the count of source-origin operations;
- remux-only never produces an encoder operation;
- the planner never produces an output with zero audio streams;
- policy classification and plan construction do not panic for arbitrary unknown
  codec/profile strings.

At least six `insta` snapshots cover add, replace, only-new, remux-only, mixed
metadata/dispositions, and a typed failure report. Snapshots show domain plans,
not FFmpeg arguments; argument snapshots belong to M3.

## Definition of Done

M2 is complete when:

- FFprobe JSON parsing and process invocation are implemented;
- the domain model, compatibility policies, and pure planner follow this contract;
- at least 20 policy/planner unit tests exist (the design targets more than 40);
- planner snapshots are reviewed and committed;
- tests perform no runtime media, filesystem, or FFmpeg access;
- `sonicmux-core` remains free of process, async-runtime, CLI, TUI, and Tauri
  dependencies;
- `cargo fmt --all --check` passes;
- `cargo clippy --workspace --all-targets --all-features -- -D warnings` passes;
- `cargo test --workspace` passes.

## Approval points

Approval of this M2 design accepts these refinements:

1. use `-of json` instead of deprecated `-print_format json`;
2. change the general `Bitrate` newtype from `u32` to `u64`;
3. retain source language tags verbatim rather than falsely normalizing all of
   them as BCP 47;
4. keep Samsung, LG, and DLNA as explicitly conservative generic baselines until
   sourced model-specific rules are added;
5. reject completely unknown codecs by default, with an explicit custom-policy
   fallback switch;
6. resolve language selectors outside the planner to an exact stream index;
7. defer the complete async `MediaBackend` trait to M3 rather than adding an
   unimplemented execution method in M2.
