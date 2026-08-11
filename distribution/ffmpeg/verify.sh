#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 2 ]]; then
  echo "usage: $0 FFMPEG_PATH FFPROBE_PATH" >&2
  exit 64
fi

ffmpeg_path=$1
ffprobe_path=$2
script_directory=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
expected_version=$(jq -er '.version' "$script_directory/manifest.json")

ffmpeg_version=$($ffmpeg_path -version)
ffprobe_version=$($ffprobe_path -version)
ffmpeg_first_line=$(printf '%s\n' "$ffmpeg_version" | head -n 1)
ffprobe_first_line=$(printf '%s\n' "$ffprobe_version" | head -n 1)

for first_line in "$ffmpeg_first_line" "$ffprobe_first_line"; do
  if [[ "$first_line" != *"ffmpeg version $expected_version"* && \
        "$first_line" != *"ffprobe version $expected_version"* ]]; then
    echo "unexpected FFmpeg tool version: $first_line" >&2
    exit 1
  fi
done

configuration=$(printf '%s\n' "$ffmpeg_version" | sed -n 's/^configuration: //p')
for required_flag in --disable-gpl --disable-nonfree --disable-network --disable-autodetect; do
  if [[ " $configuration " != *" $required_flag "* ]]; then
    echo "missing required configure flag $required_flag" >&2
    exit 1
  fi
done
for forbidden_flag in --enable-gpl --enable-nonfree; do
  if [[ " $configuration " == *" $forbidden_flag "* ]]; then
    echo "forbidden configure flag $forbidden_flag" >&2
    exit 1
  fi
done

decoders=$($ffmpeg_path -hide_banner -decoders)
encoders=$($ffmpeg_path -hide_banner -encoders)
formats=$($ffmpeg_path -hide_banner -formats)

for decoder in dca truehd; do
  if ! grep -Eq "(^|[[:space:]])$decoder([[:space:]]|$)" <<<"$decoders"; then
    echo "required decoder is unavailable: $decoder" >&2
    exit 1
  fi
done
for encoder in ac3 eac3 aac; do
  if ! grep -Eq "(^|[[:space:]])$encoder([[:space:]]|$)" <<<"$encoders"; then
    echo "required encoder is unavailable: $encoder" >&2
    exit 1
  fi
done
if ! grep -Eq '^ D.*matroska' <<<"$formats" || ! grep -Eq '^  E.*matroska' <<<"$formats"; then
  echo "Matroska demuxer or muxer is unavailable" >&2
  exit 1
fi

printf 'Verified FFmpeg %s sidecar capabilities\n' "$expected_version"
