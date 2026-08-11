# Releasing SonicMux

The automated release contract is implemented by
`.github/workflows/release.yml`. A manual dispatch builds and validates the full
matrix without publishing. Only an annotated `vMAJOR.MINOR.PATCH` tag on `main`
can create a draft release and enter the protected `release` environment.

## Before the tag

1. Merge a green release PR into `main`.
2. Confirm `distribution/check-version.sh vMAJOR.MINOR.PATCH` succeeds.
3. Run the release workflow manually and inspect every native artifact.
4. Complete `docs/testing/m8-release.md`, starting with Windows MSI and NSIS.
5. Create a short-lived crates.io token scoped to publishing the six SonicMux
   packages and store it as the `CARGO_REGISTRY_TOKEN` environment secret.
6. Obtain explicit maintainer approval for the irreversible publication step.

The first crates.io release uses a token because trusted publishing can only be
configured after the crate exists. Rotate/delete the token immediately after
the release, then configure trusted publishing for subsequent versions.

## Tag and protected publication

Create an annotated tag only from the reviewed `main` commit:

```console
git tag -a v0.1.0 -m "SonicMux 0.1.0"
git push origin v0.1.0
```

The tag workflow verifies release identity, builds the CLI/TUI and GUI matrix,
builds every bundled FFmpeg pair from the pinned source, runs a real conversion,
creates SPDX SBOM and checksums, attests the files, and creates a draft GitHub
Release. The protected `release` job then publishes crates in dependency order
and makes the GitHub Release public.

If any registry upload fails, stop: crates.io versions are immutable. Do not
retag or overwrite a release asset. Diagnose the partial state and prepare a new
patch version if the published contents cannot be completed safely.

## Platform signing

Version `0.1.0` intentionally ships unsigned previews. The following names are
reserved for a later signing stage and must live in a protected environment:

- `WINDOWS_CERTIFICATE_BASE64` and `WINDOWS_CERTIFICATE_PASSWORD` for an
  Authenticode PFX;
- `APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY`,
  `APPLE_ID`, `APPLE_PASSWORD`, and `APPLE_TEAM_ID` for Developer ID signing and
  notarization.

Assets must keep `unsigned-preview` in their name unless CI verifies an
Authenticode signature or Apple notarization ticket. GitHub attestations are
reported separately and never described as platform signing.

## Recovery and verification

The release is complete only when crates install from crates.io, all clean-host
checks pass, `sha256sum -c` succeeds, and `gh attestation verify` identifies
`gl-epka/sonicmux` as the source repository. Record evidence in the release
validation document and revoke the initial crates.io token.
