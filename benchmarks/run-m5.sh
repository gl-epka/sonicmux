#!/usr/bin/env sh
set -eu

input=${1:?usage: run-m5.sh INPUT.mkv [JOBS]}
jobs=${2:-4}
runs=${SONICMUX_BENCH_RUNS:-5}
warmup=${SONICMUX_BENCH_WARMUP:-1}

test -f "$input" && test ! -L "$input"
case "$jobs" in
  '' | *[!0-9]* | 0) exit 2 ;;
esac
[ "$jobs" -le 64 ] || exit 2
case "$runs:$warmup" in
  *[!0-9:]*) exit 2 ;;
esac
[ "$runs" -gt 0 ] || exit 2
command -v hyperfine >/dev/null 2>&1 || {
  printf '%s\n' 'hyperfine is required' >&2
  exit 3
}

repository=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd -P)
binary="$repository/target/release/sonicmux"
reset="$repository/benchmarks/reset-m5-output.sh"
cargo build --release -p sonicmux-cli --manifest-path "$repository/Cargo.toml"

temporary_base=${TMPDIR:-/tmp}
test -d "$temporary_base"
temporary_base=$(CDPATH= cd -- "$temporary_base" && pwd -P)
root=$(mktemp -d "$temporary_base/sonicmux-m5-bench.XXXXXX")
case "$root" in
  "$temporary_base"/sonicmux-m5-bench.*) ;;
  *) exit 4 ;;
esac
case "$root$binary$reset" in
  *"'"*)
    printf '%s\n' "benchmark paths cannot contain a single quote" >&2
    exit 4
    ;;
esac

mkdir "$root/inputs" "$root/out-1" "$root/out-n"
for index in 1 2 3 4; do
  cp "$input" "$root/inputs/movie-$index.mkv"
done

inputs="'$root/inputs/movie-1.mkv' '$root/inputs/movie-2.mkv' '$root/inputs/movie-3.mkv' '$root/inputs/movie-4.mkv'"
prepare="'$reset' '$root' '$root/out-1'; '$reset' '$root' '$root/out-n'"
sequential="'$binary' convert $inputs --jobs 1 --output-dir '$root/out-1' --quiet"
parallel="'$binary' convert $inputs --jobs $jobs --output-dir '$root/out-n' --quiet"

hyperfine \
  --warmup "$warmup" \
  --runs "$runs" \
  --export-json "$root/results.json" \
  --prepare "$prepare" \
  "$sequential" \
  "$parallel"

printf '%s\n' "Benchmark workspace retained at: $root"
