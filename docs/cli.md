# SonicMux CLI

- Status: Implemented M4 contract
- Date: 2026-08-10

This document describes the implemented M4 CLI contract. Exact generated
wrapping and option ordering are locked by `trycmd` snapshots; names, meanings,
conflicts, output channels, and exit codes require an approved design change.

## Top-level help

```text
Make MKV audio playable on TVs without re-encoding video.

Usage: sonicmux [OPTIONS] <COMMAND>

Commands:
  probe        Inspect streams, chapters, attachments, and metadata
  convert      Convert incompatible audio or remux existing compatible audio
  scan         Find MKV files and show the actions they require
  config       Inspect and manage configuration
  presets      List and inspect device presets
  doctor       Check FFmpeg and required codec capabilities
  completions  Generate shell completion scripts
  man          Generate a manual page
  help         Print this message or the help of a command

Options:
      --config <PATH>           Use this TOML configuration file
      --ffmpeg-path <PATH>      Use this FFmpeg executable or installation directory
      --json                    Write the final result as JSON to stdout
      --json-progress           Write versioned NDJSON events to stdout [conflicts: --json]
      --color <WHEN>            Color output [default: auto] [values: auto, always, never]
      --log-file <PATH>         Also write structured diagnostic logs to a file
  -v, --verbose...              Increase diagnostic verbosity
  -q, --quiet                   Hide non-error human output
  -h, --help                    Print help
  -V, --version                 Print version
```

`NO_COLOR` disables color when `--color` is not explicit. `RUST_LOG` configures
diagnostic filters. Logs and human diagnostics go to stderr. Data and JSON go to
stdout. `--json` emits one final JSON document; `--json-progress` emits versioned
newline-delimited events including one terminal batch event. Progress bars are
disabled when stderr is not a terminal or either JSON mode is active.

Global configuration environment variables use the `SONICMUX_` prefix, for
example `SONICMUX_FFMPEG_PATH`, `SONICMUX_PROFILE`, and `SONICMUX_CODEC`.

## Probe

```text
Inspect streams, chapters, attachments, and metadata.

Usage: sonicmux probe [OPTIONS] <INPUT>...

Arguments:
  <INPUT>...                 MKV files to inspect

Options:
      --compact              Show one summary row per file
  -h, --help                 Print help
```

Human output lists stream index, type, codec/profile, channels/layout, bitrate,
language, title, dispositions, and timing information. JSON uses a versioned
schema and represents unknown fields as null or explicit unknown variants.

## Convert

```text
Convert incompatible audio or remux existing compatible audio.

Usage: sonicmux convert [OPTIONS] <INPUT>...

Arguments:
  <INPUT>...                    MKV files, directories, or glob patterns

Compatibility and audio:
      --profile <PROFILE>       Device profile [default: generic-tv]
                                [values: generic-tv, samsung, lg, dlna, custom]
      --codec <CODEC>           Target audio codec [default: ac3]
                                [values: ac3, eac3, aac]
      --bitrate <RATE>          Target bitrate, for example 640k [default: 640k]
      --channels <LAYOUT>       Output layout [default: keep-up-to-5.1]
                                [values: keep-up-to-5.1, stereo, 5.1]
      --mode <MODE>             Audio output mode [default: add]
                                [values: add, replace, only-new]
      --remux-only              Make an existing compatible audio track default;
                                do not encode audio
      --default-audio <SELECTOR>
                                Track for --remux-only: stream index, language,
                                or `first-compatible` [default: first-compatible]

Input discovery:
  -r, --recursive                Recurse into input directories
      --include <GLOB>           Include matching names [default: *.mkv]
      --exclude <GLOB>           Exclude matching paths; repeatable
      --follow-links             Follow symbolic links during discovery

Output and safety:
  -o, --output <PATH>            Output path; requires exactly one input file
      --output-dir <DIR>         Put outputs in this directory
      --dry-run                  Probe and print plans without writing files
  -h, --help                     Print help
```

Directories require `--recursive` to descend below their first level. Glob
patterns are expanded by SonicMux as well as by shells, so behavior is available
on Windows. Only `.mkv` inputs are accepted in the initial product.

`--remux-only` conflicts with `--codec`, `--bitrate`, `--channels`, and `--mode`.
It fails when its selector finds no compatible track. It maps all streams and
metadata with copy and changes only planned dispositions/container metadata.

`--output` and `--output-dir` are mutually exclusive. Replacing and in-place
transactions are deliberately deferred until they have a separately approved
recoverable design.

Default collision behavior is:

1. matching valid result: skip;
2. existing invalid or non-matching result: fail;
3. no result: write a sibling temporary file, validate it, then rename it to
   `<stem>.sonicmux.mkv`.

Dry-run performs discovery, FFprobe, config merging, compatibility checks, and
planning. It does not invoke FFmpeg for execution, create output directories, or
write temporary files.

## Scan

```text
Find MKV files and show the actions they require.

Usage: sonicmux scan [OPTIONS] <PATH>...

Arguments:
  <PATH>...                    Files, directories, or glob patterns

Options:
  -r, --recursive              Recurse into directories
      --include <GLOB>         Include matching names [default: *.mkv]
      --exclude <GLOB>         Exclude matching paths; repeatable
      --profile <PROFILE>      Device profile [default: generic-tv]
      --codec <CODEC>          Proposed target [default: ac3]
      --bitrate <RATE>         Proposed bitrate [default: 640k]
      --channels <LAYOUT>      Proposed channel layout [default: keep-up-to-5.1]
      --mode <MODE>            Proposed output mode [default: add]
      --needs-action           Show only files requiring conversion or remux
  -h, --help                   Print help
```

`scan` is read-only. It reports `compatible`, `transcode`, `remux-available`, or
`unsupported` for each file. Execution remains an explicit `convert` command.

## Config

```text
Inspect and manage configuration.

Usage: sonicmux config <COMMAND>

Commands:
  show       Print the effective merged configuration
  path       Print the active configuration path
  init       Write a documented starter configuration
  validate   Validate a configuration file without running a job
  help       Print this message or the help of a command
```

`config init` always refuses to replace an existing file. `config show
--sources` annotates each effective value with `cli`, `env`, `config`, or
`default`.

## Presets

```text
List and inspect device presets.

Usage: sonicmux presets <COMMAND>

Commands:
  list       List built-in and configured presets
  show       Show compatibility and target rules for a preset
  help       Print this message or the help of a command
```

Vendor preset output includes its conservative support assumptions and warns
that support varies by model and firmware.

## Doctor

```text
Check FFmpeg and required codec capabilities.

Usage: sonicmux doctor [OPTIONS]

Options:
      --profile <PROFILE>       Check capabilities needed by this profile
      --codec <CODEC>           Check this target encoder [default: ac3]
      --print-paths             Show resolved ffmpeg and ffprobe paths
  -h, --help                    Print help
```

Checks include executable discovery, version, matching ffmpeg/ffprobe origin,
Matroska demux/mux support, DTS/DTS-HD/TrueHD decoders, selected audio encoder,
and a writable temporary/output location when one is supplied through config.

## Completions and man page

```text
Usage: sonicmux completions <SHELL>

Arguments:
  <SHELL>  [values: bash, elvish, fish, powershell, zsh]

Usage: sonicmux man [OPTIONS]

Options:
  -o, --output <PATH>  Write the man page to a file instead of stdout
```

Both commands write generated content to stdout by default so packaging tools
can redirect it without temporary files.

## Exit codes

| Code | Meaning |
| ---: | --- |
| 0 | All requested work succeeded or was validly skipped |
| 1 | Batch completed with one or more file failures |
| 2 | Invalid command-line arguments or configuration |
| 3 | FFmpeg/FFprobe missing, incompatible, or missing a required codec |
| 4 | Input discovery or probe failed before a valid plan could be built |
| 5 | Planning failed, including unsupported media or `NothingToDo` where action was required |
| 6 | Execution, validation, or safe commit failed |
| 130 | Cancelled by the user |

For a mixed batch, code 1 takes precedence over phase-specific per-file codes;
the JSON batch report retains each file's exact failure category. Argument
parsing uses code 2. Cancellation uses 130 consistently on all target platforms
as a SonicMux contract rather than an OS-derived process status.
