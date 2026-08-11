## What changed

<!-- Describe the user-visible and architectural impact. -->

## Why

<!-- Link the issue or explain the problem. -->

## Validation

- [ ] `cargo fmt --all --check`
- [ ] `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- [ ] `cargo test --workspace --all-targets`
- [ ] GUI tests/build run when desktop files changed
- [ ] Documentation and changelog updated when user-visible behavior changed

## Safety

- [ ] No credentials, private paths, generated media, or release binaries are committed
- [ ] Output collision, cancellation, and cleanup guarantees remain intact
