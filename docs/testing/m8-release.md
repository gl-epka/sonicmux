# M8 release validation record

- Release: `v0.1.0`
- Status: awaiting release-candidate artifacts
- Operator: not yet recorded
- Date: not yet recorded
- Commit: not yet recorded

This record must be completed against artifacts produced from the reviewed
`main` commit before the annotated tag is pushed. A green build alone does not
claim these clean-host checks passed.

## Windows first gate

Record Windows edition/build, architecture, VM cleanliness, WebView2 version,
installer filename, SHA-256, and evidence paths for both MSI and NSIS.

- [ ] Install non-interactively on a VM without system FFmpeg.
- [ ] Launch the installed native window at the 760×560 minimum.
- [ ] Confirm the complete WebView renders and keyboard focus is visible.
- [ ] Confirm About SonicMux identifies FFmpeg 8.1.2 and LGPL-2.1-or-later.
- [ ] Generate an MKV fixture and complete DTS-to-AC-3 conversion.
- [ ] Confirm the bundled toolchain wins over the absent system toolchain.
- [ ] Cancel a long conversion and find no child process or private staging path.
- [ ] Uninstall and confirm the installation directory is removed.
- [ ] Repeat for Windows x86_64 and ARM64 artifacts.

## Linux and macOS gates

For x86_64 and ARM64 Linux, validate AppImage and deb launch, bundled-toolchain
selection, real conversion, cancellation cleanup, and package removal. For the
universal macOS app archive and DMG, validate both slices, bundled-toolchain
selection, conversion, cancellation cleanup, and Gatekeeper warning wording.

## Release-set verification

- [ ] Every filename and installer version matches `0.1.0`.
- [ ] `SHA256SUMS` covers every downloadable asset except itself.
- [ ] `sha256sum -c SHA256SUMS --ignore-missing` succeeds.
- [ ] `gh attestation verify` succeeds for every native asset and the SBOM.
- [ ] FFmpeg source archive, signature, signing key, license, manifests,
      configure flags, notices, and `changes.diff` are present.
- [ ] CLI/TUI archives contain both executables, completions, man page, README,
      changelog, and SonicMux license files but no FFmpeg executable.
- [ ] `cargo install sonicmux --version 0.1.0 --locked` succeeds after publication.
- [ ] `cargo install sonicmux-tui --version 0.1.0 --locked` succeeds after publication.

Attach links to the workflow run, release draft, checksums, screenshots, smoke
reports, and clean-host logs before changing this record to `passed`.
