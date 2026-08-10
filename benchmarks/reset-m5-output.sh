#!/usr/bin/env sh
set -eu

root=${1:?usage: reset-m5-output.sh BENCH_ROOT OUTPUT_DIR}
output=${2:?usage: reset-m5-output.sh BENCH_ROOT OUTPUT_DIR}

test -d "$root" && test ! -L "$root"
test -d "$output" && test ! -L "$output"
root=$(CDPATH= cd -- "$root" && pwd -P)
output=$(CDPATH= cd -- "$output" && pwd -P)
case "$output" in
  "$root"/out-1 | "$root"/out-n) ;;
  *)
    printf '%s\n' "refusing to clean output outside the benchmark root" >&2
    exit 2
    ;;
esac

find "$output" -mindepth 1 -maxdepth 1 -type f -name '*.sonicmux.mkv' -delete
