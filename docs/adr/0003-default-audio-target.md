# ADR-0003: Default to AC-3 5.1 at 640 kbit/s

- Status: Accepted
- Date: 2026-08-10

## Context

The primary use case is a television or receiver that cannot decode DTS,
DTS-HD MA, or TrueHD. The default must favor playback compatibility while
avoiding unnecessary quality loss. Existing compatible audio should not be
encoded again.

Candidate output codecs are AC-3, E-AC-3, and AAC. They differ in efficiency,
channel support, and compatibility across older televisions, receivers, DLNA
clients, and mobile devices.

The product also needs an explicit stereo downmix for devices that cannot
reliably play multichannel audio.

## Decision

The `generic-tv` default target is AC-3 at 640 kbit/s with at most 5.1 channels.
Source layouts with more than 5.1 channels are reduced to 5.1. Mono and stereo
sources retain their channel count unless the user explicitly requests another
layout. Bitrate defaults are validated against the selected codec and layout;
invalid combinations fail planning rather than being silently changed.

The initial compatibility baseline treats AC-3, AAC, and MP3 as compatible.
Profile-specific rules can narrow or extend that set. Compatibility is a typed
policy decision, not a hard-coded FFmpeg codec-name comparison.

The planner follows these rules:

- compatible audio is copied;
- incompatible DTS-family and TrueHD audio is decoded by FFmpeg and encoded to
  the configured target;
- E-AC-3 and AAC are explicit target options, not generic-TV defaults;
- `--channels stereo` is an explicit downmix request;
- no lossy-to-lossy conversion is performed solely to normalize codecs;
- an unknown codec is reported and treated as incompatible only when the active
  policy has an explicit fallback target.

Downmixing must use a documented FFmpeg channel layout or filter expression.
Automatic clipping protection or loudness normalization is not enabled by
default because either changes audio beyond the compatibility requirement.

For DTS-HD MA and TrueHD, FFmpeg is asked to decode the selected stream. SonicMux
does not intentionally select a lossy DTS core when the backend can decode the
full stream. `doctor` reports decoder availability before execution.

The three output modes have precise audio semantics:

- `add`: copy all original audio and append one compatible derivative for every
  incompatible audio stream. New derivatives receive copied language/title
  metadata, a generated title suffix, and become default according to the
  disposition rule in the plan.
- `replace`: copy compatible audio; replace every incompatible audio stream with
  its compatible derivative while retaining its relative audio order and
  metadata.
- `only-new`: omit all original audio and keep only derivatives of incompatible
  streams. If no derivative is needed, planning returns `NothingToDo` rather
  than creating a silent file.

The default-disposition rule for `add` is deterministic: if an incompatible
source track was default, its derivative becomes default and the source loses
only the `default` flag in the output. If no source audio was default, the first
new derivative becomes default. Other disposition flags are preserved where
Matroska and FFmpeg support them.

The remux fast path is enabled. If at least one compatible audio stream already
exists, the user may select it (or accept the deterministic first preferred
stream) as default and remux without audio transcoding. Automatic remux never
silently replaces a requested `add` or `replace` conversion; it is a distinct
plan action exposed in dry-run output.

## Consequences

- The out-of-box result targets the widest practical legacy-TV compatibility.
- Lossless sources become lossy unless the remux fast path can use an existing
  compatible track; dry-run must make that loss visible.
- Multichannel AAC and E-AC-3 remain available for device-specific profiles.
- Stream metadata/disposition behavior is part of planner snapshots and cannot
  be left to FFmpeg defaults.
- The exact `only-new` semantics and `add` disposition behavior require explicit
  approval with M0 because both names admit other interpretations.
