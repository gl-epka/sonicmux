# Contributing to SonicMux

Thank you for helping improve SonicMux.

## Development setup

Install Rust 1.88 or newer, Node 24, FFmpeg, and the native Tauri prerequisites
for your operating system. Then run:

```console
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets
cd crates/gui && npm ci && npm test && npm run build
```

Real-media tests are opt-in because they launch FFmpeg:

```console
SONICMUX_RUN_FFMPEG_TESTS=1 cargo test -p sonicmux-runtime --test ffmpeg_execution
```

## Changes

- Open an issue before a large behavioral or architectural change.
- Keep one concern per pull request.
- Use Conventional Commit subjects such as `feat:`, `fix:`, `docs:`, and `test:`.
- Add tests for changed behavior and update user-facing documentation.
- Do not commit generated media, release binaries, credentials, or private paths.

Contributors retain copyright in their work and license contributions under the
project's `MIT OR Apache-2.0` terms. No CLA or DCO sign-off is required.
