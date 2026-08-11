#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 ]]; then
  echo "usage: $0 OUTPUT_DIRECTORY" >&2
  exit 64
fi

output_directory=$1
script_directory=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
manifest_path="$script_directory/manifest.json"

mkdir -p -- "$output_directory"
output_directory=$(cd -- "$output_directory" && pwd)

version=$(jq -er '.version' "$manifest_path")
source_url=$(jq -er '.source_url' "$manifest_path")
source_sha256=$(jq -er '.source_sha256' "$manifest_path")
signature_url=$(jq -er '.signature_url' "$manifest_path")
signature_sha256=$(jq -er '.signature_sha256' "$manifest_path")
signing_key_url=$(jq -er '.signing_key_url' "$manifest_path")
signing_key_sha256=$(jq -er '.signing_key_sha256' "$manifest_path")
signing_key_fingerprint=$(jq -er '.signing_key_fingerprint' "$manifest_path")

archive="$output_directory/ffmpeg-$version.tar.xz"
signature="$archive.asc"
signing_key="$output_directory/ffmpeg-devel.asc"
keyring="$output_directory/gnupg"
source_tree="$output_directory/ffmpeg-$version"

download() {
  local url=$1
  local destination=$2
  local temporary="$destination.partial"
  if [[ -e "$destination" || -e "$temporary" ]]; then
    echo "refusing to replace existing download path: $destination" >&2
    exit 1
  fi
  curl --fail --location --retry 5 --retry-all-errors --connect-timeout 30 \
    --proto '=https' --tlsv1.2 \
    --output "$temporary" "$url"
  mv -- "$temporary" "$destination"
}

verify_sha256() {
  local expected=$1
  local path=$2
  local actual
  if command -v sha256sum >/dev/null 2>&1; then
    actual=$(sha256sum "$path" | awk '{print $1}')
  else
    actual=$(shasum -a 256 "$path" | awk '{print $1}')
  fi
  if [[ "$actual" != "$expected" ]]; then
    echo "SHA-256 mismatch for $path: expected $expected, got $actual" >&2
    exit 1
  fi
}

download "$source_url" "$archive"
download "$signature_url" "$signature"
download "$signing_key_url" "$signing_key"
verify_sha256 "$source_sha256" "$archive"
verify_sha256 "$signature_sha256" "$signature"
verify_sha256 "$signing_key_sha256" "$signing_key"

if ! command -v gpg >/dev/null 2>&1; then
  echo "gpg is required to verify the FFmpeg release signature" >&2
  exit 1
fi

mkdir -- "$keyring"
case "${OSTYPE:-}" in
  msys*|cygwin*) ;;
  *) chmod 700 "$keyring" ;;
esac
GNUPGHOME="$keyring" gpg --batch --import "$signing_key"
imported_fingerprint=$(GNUPGHOME="$keyring" gpg --batch --with-colons \
  --fingerprint ffmpeg-devel@ffmpeg.org | awk -F: '$1 == "fpr" {print $10; exit}')
if [[ "$imported_fingerprint" != "$signing_key_fingerprint" ]]; then
  echo "unexpected FFmpeg signing key fingerprint: $imported_fingerprint" >&2
  exit 1
fi
GNUPGHOME="$keyring" gpg --batch --verify "$signature" "$archive"

if [[ -e "$source_tree" ]]; then
  echo "refusing to replace existing source tree: $source_tree" >&2
  exit 1
fi
tar -xJf "$archive" -C "$output_directory"
printf 'Verified FFmpeg %s source at %s\n' "$version" "$archive"
