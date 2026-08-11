# SonicMux FFmpeg sidecar build

SonicMux GUI release bundles use FFmpeg 8.1.2 built from the official source
archive recorded in `manifest.json`. The scripts never resolve a mutable latest
release and never use a prebuilt FFmpeg executable as release input.

## Build

The build host needs a C toolchain, GNU Make, curl, xz, GnuPG, and jq:

```console
distribution/ffmpeg/fetch.sh target/ffmpeg-source
distribution/ffmpeg/build.sh target/ffmpeg-source/ffmpeg-8.1.2 target/ffmpeg-install
distribution/ffmpeg/verify.sh target/ffmpeg-install/bin/ffmpeg target/ffmpeg-install/bin/ffprobe
distribution/ffmpeg/stage-sidecars.sh target/ffmpeg-install/bin x86_64-unknown-linux-gnu
```

On Windows these scripts run in an MSYS2 UCRT64 or CLANGARM64 shell. The native
toolchain is selected by that environment; the same configuration flags and
verification contract apply.

`build.sh` writes `build-manifest.json` next to the installed programs. Release
jobs preserve that file, the source archive, detached signature, public signing
key, FFmpeg license files, and a zero-length `changes.diff` proving that the
official source was built without source modifications.

The resulting FFmpeg programs are separate works invoked as child processes.
They are not linked into SonicMux. SonicMux itself remains `MIT OR Apache-2.0`.
See `THIRD_PARTY_LICENSES.md` and ADR-0002 for the distribution policy.
