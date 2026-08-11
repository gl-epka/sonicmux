# M8 design: release engineering and project presentation

- Status: Accepted
- Date: 2026-08-11
- Target release: `v0.1.0`

## Scope

M8 turns the repository built in M0–M7 into the first public SonicMux release.
It publishes the command-line packages to crates.io, attaches native CLI, TUI,
and GUI artifacts to one GitHub Release, supplies version-pinned FFmpeg
sidecars in GUI bundles, and adds the project governance and security files
expected from a public open-source repository.

M8 does not expand the media model beyond MKV, add automatic downloads or an
updater, add an overwrite/in-place transaction, or claim platform code signing
that the project does not possess. The first GUI artifacts are explicitly
labelled unsigned previews. Their checksums and build provenance are still
published and verifiable.

## Version and release identity

The first release is SemVer `0.1.0`, represented by one annotated Git tag
`v0.1.0`. The workspace version, Tauri version, changelog heading, tag, archive
names, installer versions, and crates.io versions must agree. Release automation
fails before building when any value differs.

The tag is created only from a clean, green commit on `main`. A tag workflow
creates a draft GitHub Release first and publishes it only after every required
artifact and validation job succeeds.

## crates.io package set

The command-line package is renamed from `sonicmux-cli` to `sonicmux`; its
binary remains `sonicmux`. This gives users the expected installation command:

```console
cargo install sonicmux --locked
cargo install sonicmux-tui --locked
```

Registry packages are published in dependency order:

1. `sonicmux-core`;
2. `sonicmux-backend`;
3. `sonicmux-ffmpeg`;
4. `sonicmux-runtime`;
5. `sonicmux`;
6. `sonicmux-tui`.

All registry packages contain repository, homepage, readme, license, keywords,
categories, and MSRV metadata. Registry dependencies keep both a local `path`
and the exact compatible `version`. `sonicmux-gui` and `xtask` are explicitly
non-publishable.

Before the irreversible upload, every package runs `cargo package --list`,
`cargo publish --dry-run`, and a build from the generated crate archive. The
initial release uses a short-lived scoped crates.io token because trusted OIDC
publishing can only be configured after a crate exists. Future releases move to
crates.io trusted publishing through a protected GitHub `release` environment.

## Native application matrix

Release archives contain both `sonicmux` and `sonicmux-tui`, generated shell
completions, the section-1 manual, README, changelog, and SonicMux licenses.
CLI/TUI archives never include FFmpeg.

| Platform | CLI/TUI | GUI bundle |
| --- | --- | --- |
| Linux x86_64 | GNU and musl tar.xz | AppImage and deb |
| Linux aarch64 | GNU and musl tar.xz | AppImage and deb |
| Windows x86_64 | zip | NSIS setup and MSI |
| Windows aarch64 | zip | NSIS setup and MSI |
| macOS universal | tar.xz | DMG and application archive |

Native GitHub-hosted runners are used for Linux, Windows, and macOS ARM64.
Cross-compilation is allowed only for the musl CLI/TUI variants and the second
macOS slice used to create a universal binary. GUI installers are built on the
target operating system.

## FFmpeg sidecar supply chain

GUI bundles contain `ffmpeg` and `ffprobe` built by the release workflow from a
pinned official FFmpeg 8.1.2 source archive. SonicMux does not consume mutable
`latest` URLs or copy opaque third-party executable archives.

The build configuration must:

- omit `--enable-gpl` and `--enable-nonfree`;
- disable network protocols and external-library autodetection;
- avoid GPL and non-free external libraries;
- retain Matroska demux/mux, DTS and TrueHD decoding, native AC-3, E-AC-3, and
  AAC encoding, audio resampling/downmix support, file and pipe protocols, and
  the `ffmpeg` and `ffprobe` programs;
- produce the target-suffixed names required by Tauri `externalBin`;
- retain the ordinary configured → sidecar → system resolution order.

The source manifest records URL, source SHA-256, release-signing identity,
configure arguments, compiler, target, and resulting executable hashes. A
release includes the exact source archive, build manifests, license texts,
`changes.diff`, and `THIRD_PARTY_LICENSES.md` on the same GitHub Release as the
GUI binaries.

Every sidecar pair must pass:

1. checksum verification;
2. `ffmpeg -version` and `ffprobe -version` agreement;
3. an audit that configuration contains neither GPL nor non-free enablement;
4. the SonicMux capability request for Matroska, DTS, TrueHD, and the three
   target encoders;
5. a real generated DTS-to-AC-3 conversion with stream-copy video validation.

## Automation boundaries

The workflow is deliberately split into reusable, reviewable stages rather
than giving one opaque release tool all credentials:

- `ci.yml` remains the required PR and `main` gate;
- `security.yml` runs weekly and on demand with dependency, license, and
  advisory checks;
- `release.yml` runs on `v*` tags, builds the full matrix, creates checksums,
  SBOMs, attestations, and the draft GitHub Release;
- a protected publish job uploads crates in dependency order and makes the
  GitHub Release public only after the registry and artifact checks pass;
- Dependabot maintains Cargo, npm, and GitHub Actions dependencies in grouped
  pull requests.

Actions that receive write or OIDC permissions are pinned to immutable commit
SHAs. Build jobs have read-only repository permissions. Release publication
uses `contents: write`; provenance uses `id-token: write` and
`attestations: write`; crates publishing is isolated in the `release`
environment.

`SHA256SUMS` covers every downloadable release asset except itself. GitHub
artifact attestations cover the same immutable files and the SBOM. Release
documentation shows both `sha256sum -c` and `gh attestation verify` commands.

## Installer validation

The Windows installer remains the first release gate. A clean Windows VM:

- installs MSI and NSIS packages non-interactively;
- launches the installed native window at the 760×560 minimum;
- verifies that the rendered WebView is complete;
- verifies the bundled toolchain wins without system FFmpeg;
- generates an MKV fixture and completes a real conversion;
- cancels a deliberately long conversion and finds no child process or private
  staging directory;
- uninstalls the application and checks the install directory is removed.

Equivalent package launch, bundled capability, and conversion checks run for
macOS and Linux. The extended visual, picker, high-contrast, and display-scale
checklist remains recorded separately and is not silently represented as an
automated check.

## Platform signing policy

Version 0.1.0 ships without Authenticode or Apple Developer ID credentials.
Windows and macOS GUI assets are named and documented as unsigned previews and
may trigger SmartScreen or Gatekeeper warnings. GitHub provenance is not called
platform code signing and does not remove those warnings.

The workflow contains conditional signing/notarization stages with documented
secret names, but absence of those secrets must never cause a false `signed`
label. A later release may remove the preview label only after CI verifies the
Windows signature and Apple notarization ticket.

## Repository presentation and governance

M8 adds the dual license texts, Keep a Changelog history, contributing guide,
Contributor Covenant, security policy, issue forms, pull-request template,
editor settings, attributes, release-oriented README badges, installation and
verification instructions, FAQ, and acknowledgements.

Repository metadata is updated to include the project homepage/description and
the `rust`, `ffmpeg`, `mkv`, `dts`, `ac3`, `tui`, `tauri`, and `cli` topics.
`main` receives required status checks and disallows force pushes and deletion.

## Definition of Done

M8 is complete when:

- format, Clippy with warnings denied, tests, docs, MSRV, frontend tests, real
  FFmpeg integration, package dry-runs, security checks, and native GUI builds
  are green;
- the six Rust packages are visible on crates.io at `0.1.0` and install from
  the registry;
- annotated tag `v0.1.0` points at the reviewed `main` commit;
- the public GitHub Release contains every required target, sidecar source and
  notice artifact, SBOM, checksums, and release notes;
- downloaded CLI/TUI archives and installed GUI packages pass their clean-host
  smoke tests;
- `SHA256SUMS` and GitHub attestations verify after downloading from the public
  Release;
- the Windows-first validation record identifies the release, environment,
  toolchain, operator, date, and evidence;
- README installation commands and links refer only to artifacts that actually
  exist.

Release publication is irreversible external state. It occurs after the M8 PR
is reviewed, merged, and the user explicitly authorizes the final publish step.
