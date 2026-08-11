#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 ]]; then
  echo "usage: $0 vMAJOR.MINOR.PATCH" >&2
  exit 64
fi

release_tag=$1
if [[ ! "$release_tag" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "release tag must have the form vMAJOR.MINOR.PATCH: $release_tag" >&2
  exit 1
fi
release_version=${release_tag#v}
repository_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
cd "$repository_root"

workspace_versions=$(cargo metadata --no-deps --format-version 1 \
  | jq -r '.packages[].version' | sort -u)
if [[ "$workspace_versions" != "$release_version" ]]; then
  echo "workspace package versions do not all equal $release_version:" >&2
  printf '%s\n' "$workspace_versions" >&2
  exit 1
fi

tauri_version=$(jq -er '.version' crates/gui/src-tauri/tauri.conf.json)
if [[ "$tauri_version" != "$release_version" ]]; then
  echo "Tauri version $tauri_version does not equal $release_version" >&2
  exit 1
fi

if ! grep -Eq "^## \[$release_version\] - [0-9]{4}-[0-9]{2}-[0-9]{2}$" CHANGELOG.md; then
  echo "CHANGELOG.md has no release heading for $release_version" >&2
  exit 1
fi

printf 'Verified release version %s\n' "$release_version"
