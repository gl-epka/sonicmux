# ADR-0002: Use hybrid FFmpeg discovery and distribution

- Status: Accepted
- Date: 2026-08-10

## Context

SonicMux is intended to use the dual license `MIT OR Apache-2.0`. FFmpeg is a
separate project whose effective license depends on its build configuration.
Calling a user-installed executable and redistributing an executable have
different operational and compliance requirements.

CLI-first development should not make Rust installation depend on downloading a
large media tool. The GUI, however, needs a predictable first-run experience.

Target platforms are Linux, Windows, and macOS, with Windows as the first manual
GUI validation target.

## Decision

Adopt a hybrid distribution policy:

1. CLI and TUI use system FFmpeg by default.
2. `--ffmpeg-path PATH` and the corresponding configuration value may point to a
   specific `ffmpeg` executable or installation directory.
3. Resolution order is explicit CLI value, environment value, config value,
   bundled sidecar, then `PATH`. CLI/TUI releases normally have no sidecar, so
   the last step is `PATH`.
4. GUI installers bundle a version-pinned FFmpeg sidecar where release and
   licensing work permits it. If the sidecar is absent or unusable, the GUI
   offers system discovery and a clear setup screen rather than silently
   downloading or executing anything.
5. SonicMux does not download or update FFmpeg automatically in M0-M8.

Bundled builds must be redistributable LGPL builds. They must not enable GPL or
non-free components. Each installer that carries FFmpeg must include:

- the applicable FFmpeg license notices;
- `THIRD_PARTY_LICENSES.md`;
- the exact build configuration and version;
- a durable offer or link to the corresponding source code;
- a notice that FFmpeg is a separate work.

Release automation must verify the sidecar checksum and execute the capability
check from ADR-0001. Sidecar filenames and Tauri `externalBin` target suffixes
are platform-specific release inputs, not domain concepts.

Installation guidance for a missing system dependency will cover `winget`,
Homebrew, and the major Linux package managers, while noting that distribution
builds can expose different codecs.

This ADR records an engineering policy, not legal advice. The exact FFmpeg build
and notices must receive a release-time license review before the first bundled
GUI artifact is published.

## Consequences

- `cargo install sonicmux` stays small and does not unexpectedly install FFmpeg.
- GUI installers are larger but can provide a predictable codec set.
- Release engineering must produce, verify, and document sidecars for every GUI
  target.
- System installs can vary, so `sonicmux doctor` is part of support diagnostics.
- A user-supplied path always remains available for managed or offline systems.
- GUI capability permissions can remain narrow: only the selected input/output
  paths and the packaged sidecar need access.
