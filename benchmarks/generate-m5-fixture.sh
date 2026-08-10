#!/usr/bin/env sh
set -eu

output=${1:?usage: generate-m5-fixture.sh OUTPUT.mkv}

ffmpeg -v error -y \
  -f lavfi -i 'testsrc2=size=1280x720:rate=30:duration=20' \
  -f lavfi -i 'sine=frequency=1000:sample_rate=48000:duration=20' \
  -map 0:v:0 -map 1:a:0 \
  -c:v mpeg4 -q:v 2 \
  -c:a dca -strict experimental -b:a 768k -ac 6 \
  -metadata:s:a:0 language=eng -metadata:s:a:0 title=Main \
  -disposition:a:0 default -shortest -f matroska "$output"
