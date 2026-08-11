#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 2 ]]; then
  echo "usage: $0 TARGET_TRIPLE OUTPUT_DIRECTORY" >&2
  exit 64
fi

target_triple=$1
output_directory=$2
repository_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
version=$(sed -n 's/^version = "\([^"]*\)"/\1/p' "$repository_root/Cargo.toml" | head -n 1)
binary_directory="$repository_root/target/$target_triple/release"
archive_name="sonicmux-v$version-$target_triple.tar.xz"
archive_path="$output_directory/$archive_name"

if [[ -z "$version" ]]; then
  echo "workspace version was not found" >&2
  exit 1
fi
if [[ ! -x "$binary_directory/sonicmux" || ! -x "$binary_directory/sonicmux-tui" ]]; then
  echo "release binaries were not found in $binary_directory" >&2
  exit 1
fi
if [[ -e "$archive_path" ]]; then
  echo "refusing to replace existing archive: $archive_path" >&2
  exit 1
fi

mkdir -p -- "$output_directory"
staging_root=$(mktemp -d "${TMPDIR:-/tmp}/sonicmux-package.XXXXXX")
trap 'rm -rf -- "$staging_root"' EXIT
package_directory="$staging_root/sonicmux-v$version-$target_triple"
mkdir -p -- "$package_directory/completions"

cp -- "$binary_directory/sonicmux" "$package_directory/sonicmux"
cp -- "$binary_directory/sonicmux-tui" "$package_directory/sonicmux-tui"
cp -- "$repository_root/README.md" "$repository_root/CHANGELOG.md" \
  "$repository_root/LICENSE-APACHE" "$repository_root/LICENSE-MIT" \
  "$package_directory/"

for shell_name in bash fish powershell zsh; do
  "$binary_directory/sonicmux" completions "$shell_name" \
    > "$package_directory/completions/sonicmux.$shell_name"
done
"$binary_directory/sonicmux" man --output "$package_directory/sonicmux.1"

tar -cJf "$archive_path" -C "$staging_root" "$(basename "$package_directory")"
printf '%s\n' "$archive_path"
