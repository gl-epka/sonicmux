#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 2 ]]; then
  echo "usage: $0 SOURCE_DIRECTORY INSTALL_DIRECTORY" >&2
  exit 64
fi

source_directory=$(cd -- "$1" && pwd)
install_directory=$2
script_directory=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
manifest_path="$script_directory/manifest.json"
flags_path="$script_directory/configure-flags.txt"

if [[ -d "$install_directory" && -n "$(ls -A -- "$install_directory")" ]]; then
  echo "refusing to replace non-empty install directory: $install_directory" >&2
  exit 1
fi
mkdir -p -- "$install_directory"
install_directory=$(cd -- "$install_directory" && pwd)

configure_flags=()
while IFS= read -r configure_flag; do
  if [[ -n "$configure_flag" ]]; then
    configure_flags+=("$configure_flag")
  fi
done < "$flags_path"
configure_flags+=("--prefix=$install_directory")

case "${OSTYPE:-}" in
  darwin*)
    configure_flags+=("--cc=${CC:-clang}")
    ;;
  msys*|cygwin*)
    configure_flags+=("--target-os=mingw32")
    ;;
esac

if [[ -n "${SONICMUX_FFMPEG_EXTRA_FLAGS:-}" ]]; then
  read -r -a extra_flags <<<"$SONICMUX_FFMPEG_EXTRA_FLAGS"
  configure_flags+=("${extra_flags[@]}")
fi

pushd "$source_directory" >/dev/null
./configure "${configure_flags[@]}"
make -j "${SONICMUX_BUILD_JOBS:-2}"
make install
popd >/dev/null

ffmpeg_path="$install_directory/bin/ffmpeg"
ffprobe_path="$install_directory/bin/ffprobe"
if [[ "${OSTYPE:-}" == msys* || "${OSTYPE:-}" == cygwin* ]]; then
  ffmpeg_path="$ffmpeg_path.exe"
  ffprobe_path="$ffprobe_path.exe"
fi

sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  else
    shasum -a 256 "$1" | awk '{print $1}'
  fi
}

jq -n \
  --arg version "$(jq -er '.version' "$manifest_path")" \
  --arg host "$(uname -a)" \
  --arg compiler "$(command ${CC:-cc} --version | head -n 1)" \
  --arg configure "${configure_flags[*]}" \
  --arg ffmpeg_sha256 "$(sha256_of "$ffmpeg_path")" \
  --arg ffprobe_sha256 "$(sha256_of "$ffprobe_path")" \
  '{schema_version: 1, version: $version, host: $host, compiler: $compiler,
    configure: $configure, ffmpeg_sha256: $ffmpeg_sha256,
    ffprobe_sha256: $ffprobe_sha256, source_modified: false}' \
  > "$install_directory/build-manifest.json"

: > "$install_directory/changes.diff"
