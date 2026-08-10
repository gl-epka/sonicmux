# ADR-0006: Version CLI machine output independently from domain types

- Status: Accepted
- Date: 2026-08-11

## Context

M4 adds `--json` for one final result and `--json-progress` for newline-delimited
events. These formats will be consumed by scripts and later may be reused by the
TUI/GUI process boundary. Rust domain types are optimized for invariants and will
evolve as media support grows; directly deriving `Serialize` would accidentally
turn private representation changes and `Debug` strings into a public protocol.

Paths also are not necessarily UTF-8. M3 intentionally preserves native paths
through `OsString`, so JSON serialization must not make an otherwise valid media
operation fail or silently claim that a lossy path is exact.

Progress delivery is best effort, but success/failure and the final report are
authoritative. Human diagnostics and progress bars must not corrupt machine
stdout.

## Decision

`sonicmux-cli` owns explicit presentation DTOs and conversions from core,
backend, and runtime reports. Core types do not gain Serde solely for CLI output.

Every final JSON document contains a schema identifier and integer version:

```json
{
  "schema": "sonicmux.result",
  "version": 1,
  "command": "probe",
  "status": "success"
}
```

Every NDJSON event contains `schema = "sonicmux.event"`, `version = 1`, a
monotonically increasing sequence number, an event discriminator, and the
relevant bounded payload. The stream ends with exactly one terminal batch event
unless stdout itself fails. A terminal event is generated from the operation
result, never inferred from receipt of an intermediate progress message.

Field names carry units, including `_bps`, `_bytes`, `_us`, and `_milli`.
Enums use explicit stable lowercase names. Future enum values are represented by
an `unknown` form with a bounded retained value when available; no `Debug`
rendering enters the schema.

Paths serialize as objects:

```json
{
  "display": "movie.mkv",
  "native_encoding": "utf-8",
  "native_hex": "6d6f7669652e6d6b76"
}
```

`native_encoding` is `utf-8`, `unix-bytes`, or `windows-wtf16le`. For exact
UTF-8 paths, `native_hex` contains their UTF-8 bytes. Otherwise it contains the
exact Unix bytes or the exact Windows WTF-16 code units encoded little-endian.
The `display` field is intended for people and may be lossy. Platform-specific
conversion uses safe standard-library extensions behind `cfg`; no unsafe code
or locale conversion is introduced.

Final JSON is available for result-producing `probe`, `scan`, `convert`,
`config`, `presets`, and `doctor` commands. NDJSON is limited to `probe`, `scan`,
and `convert`, whose work has lifecycle events. Completion scripts and the man
page remain raw artifacts rather than being wrapped in either schema.

`--json` emits one compact document and one trailing newline. `--json-progress`
emits one compact object and newline per event. Each object is serialized into a
bounded buffer and written with `write_all`. Human result output uses stdout;
logs, diagnostics, warnings, and progress use stderr. Machine modes suppress all
human stdout.

Schema version 1 allows additive optional fields. Removing or renaming a field,
changing its unit/type, or reinterpreting its meaning requires a new version.
Snapshots and deserialization tests cover both final documents and NDJSON.

## Consequences

- Scripts receive a stable, documented protocol independent from Rust layouts.
- Non-UTF-8 paths remain identifiable and cannot break serialization.
- DTO conversion is deliberate code and adds some duplication.
- New domain variants require an explicit presentation decision and tests.
- NDJSON consumers must tolerate missing intermediate progress events but can
  rely on ordered sequence numbers and one authoritative terminal event.
- Later UIs may reuse the DTO concepts, but this ADR does not require them to
  communicate through JSON in-process.
