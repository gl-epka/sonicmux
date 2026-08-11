#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 ]]; then
  echo "usage: $0 RELEASE_DIRECTORY" >&2
  exit 64
fi

release_directory=$(cd -- "$1" && pwd)
checksum_file="$release_directory/SHA256SUMS"
if [[ -e "$checksum_file" ]]; then
  echo "refusing to replace existing checksum file: $checksum_file" >&2
  exit 1
fi

hash_file() {
  local path=$1
  local name
  name=$(basename -- "$path")
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$path" | sed "s#  .*#  $name#"
  else
    shasum -a 256 "$path" | sed "s#  .*#  $name#"
  fi
}

while IFS= read -r release_file; do
  hash_file "$release_file"
done < <(find "$release_directory" -maxdepth 1 -type f ! -name SHA256SUMS | sort) \
  > "$checksum_file"
