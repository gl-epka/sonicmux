#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 ]]; then
  echo "usage: $0 vMAJOR.MINOR.PATCH" >&2
  exit 64
fi
if [[ -z "${CARGO_REGISTRY_TOKEN:-}" ]]; then
  echo "CARGO_REGISTRY_TOKEN is required" >&2
  exit 1
fi

release_tag=$1
release_version=${release_tag#v}
repository_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
"$repository_root/distribution/check-version.sh" "$release_tag"
cd "$repository_root"

packages=(
  sonicmux-core
  sonicmux-backend
  sonicmux-ffmpeg
  sonicmux-runtime
  sonicmux
  sonicmux-tui
)

wait_for_registry() {
  local package_name=$1
  local attempt
  local registry_response
  local registry_version
  for attempt in $(seq 1 24); do
    registry_response=
    if registry_response=$(curl --fail --silent --show-error \
      --header 'User-Agent: sonicmux-release/0.1 (https://github.com/gl-epka/sonicmux)' \
      "https://crates.io/api/v1/crates/$package_name"); then
      registry_version=$(jq -r '.crate.max_version // empty' <<<"$registry_response")
      if [[ "$registry_version" == "$release_version" ]]; then
        return 0
      fi
    fi
    sleep 5
  done
  echo "$package_name $release_version did not appear on crates.io" >&2
  return 1
}

for package_name in "${packages[@]}"; do
  cargo package --locked --list -p "$package_name"
  cargo publish --locked --dry-run -p "$package_name"
  cargo publish --locked -p "$package_name"
  wait_for_registry "$package_name"
done
