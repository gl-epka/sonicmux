#!/usr/bin/env sh
set -eu

output=${1:?usage: generate-m3-fixture.sh OUTPUT.mkv}
metadata="${output}.ffmetadata.$$"
trap 'rm -f -- "$metadata"' EXIT HUP INT TERM

printf '%s\n' \
  ';FFMETADATA1' \
  'title=SonicMux M3 Fixture' \
  '[CHAPTER]' \
  'TIMEBASE=1/1000' \
  'START=0' \
  'END=2500' \
  'title=First half' \
  '[CHAPTER]' \
  'TIMEBASE=1/1000' \
  'START=2500' \
  'END=5000' \
  'title=Second half' >"$metadata"

ffmpeg -v error -y \
  -f lavfi -i 'testsrc2=size=160x90:rate=25:duration=5' \
  -f lavfi -i 'sine=frequency=1000:sample_rate=48000:duration=5' \
  -f ffmetadata -i "$metadata" \
  -map 0:v:0 -map 1:a:0 -map_metadata 2 -map_chapters 2 \
  -c:v mpeg4 -q:v 8 \
  -c:a dca -strict experimental -b:a 768k -ac 6 \
  -metadata:s:a:0 language=eng -metadata:s:a:0 title=Main \
  -disposition:a:0 default -shortest -f matroska "$output"
