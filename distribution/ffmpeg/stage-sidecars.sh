#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 2 ]]; then
  echo "usage: $0 BINARY_DIRECTORY TARGET_TRIPLE" >&2
  exit 64
fi

binary_directory=$(cd -- "$1" && pwd)
target_triple=$2
repository_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)
destination="$repository_root/crates/gui/src-tauri/binaries"
extension=

case "$target_triple" in
  *-pc-windows-*) extension=.exe ;;
esac

ffmpeg_source="$binary_directory/ffmpeg$extension"
ffprobe_source="$binary_directory/ffprobe$extension"
if [[ ! -f "$ffmpeg_source" || ! -f "$ffprobe_source" ]]; then
  echo "FFmpeg pair was not found in $binary_directory" >&2
  exit 1
fi

mkdir -p -- "$destination"
ffmpeg_destination="$destination/ffmpeg-$target_triple$extension"
ffprobe_destination="$destination/ffprobe-$target_triple$extension"
if [[ -e "$ffmpeg_destination" || -e "$ffprobe_destination" ]]; then
  echo "refusing to replace existing staged sidecars for $target_triple" >&2
  exit 1
fi
cp -- "$ffmpeg_source" "$ffmpeg_destination"
cp -- "$ffprobe_source" "$ffprobe_destination"
printf 'Staged FFmpeg sidecars for %s\n' "$target_triple"
