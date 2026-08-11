# Release sidecars

Release automation stages target-suffixed `ffmpeg` and `ffprobe` executables in
this directory immediately before Tauri bundling. Executables are ignored by
Git and must never be committed.

Use `distribution/ffmpeg/stage-sidecars.sh`; do not manually rename downloaded
binaries.
