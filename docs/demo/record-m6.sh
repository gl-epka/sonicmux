#!/usr/bin/env sh
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
temp_root=${TMPDIR:-/tmp}
demo_dir=$(mktemp -d "${temp_root%/}/sonicmux-m6-demo.XXXXXX")

case "$demo_dir" in
  "${temp_root%/}"/sonicmux-m6-demo.*) ;;
  *)
    printf '%s\n' "unexpected demo directory: $demo_dir" >&2
    exit 1
    ;;
esac

cleanup() {
  rm -f -- "${demo_dir:?}/input.mkv"
  rmdir -- "${demo_dir:?}"
}
trap cleanup EXIT HUP INT TERM

command -v cargo >/dev/null 2>&1 || {
  printf '%s\n' 'cargo is required' >&2
  exit 1
}
command -v ffmpeg >/dev/null 2>&1 || {
  printf '%s\n' 'ffmpeg is required' >&2
  exit 1
}
command -v vhs >/dev/null 2>&1 || {
  printf '%s\n' 'vhs is required: https://github.com/charmbracelet/vhs' >&2
  exit 1
}

cd "$root"
cargo build --release --locked -p sonicmux-tui

ffmpeg -v error -y \
  -f lavfi -i 'color=c=black:size=320x180:rate=24:duration=3' \
  -f lavfi -i 'sine=frequency=440:sample_rate=48000:duration=3' \
  -map 0:v:0 -map 1:a:0 \
  -c:v mpeg4 -q:v 10 -c:a flac \
  -metadata:s:a:0 language=eng -metadata:s:a:0 title='VHS demo' \
  -shortest -f matroska "${demo_dir:?}/input.mkv"

SONICMUX_M6_DEMO_INPUT="${demo_dir:?}/input.mkv"
export SONICMUX_M6_DEMO_INPUT
vhs docs/demo/m6.tape
