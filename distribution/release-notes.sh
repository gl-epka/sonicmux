#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 2 ]]; then
  echo "usage: $0 VERSION OUTPUT_FILE" >&2
  exit 64
fi

version=$1
output_file=$2
repository_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)

awk -v heading="## [$version]" '
  index($0, heading) == 1 {printing = 1; next}
  printing && /^## \[/ {exit}
  printing {print}
' "$repository_root/CHANGELOG.md" > "$output_file"

if [[ ! -s "$output_file" ]]; then
  echo "no changelog notes found for $version" >&2
  exit 1
fi

cat >> "$output_file" <<'EOF'

## Verification

Download `SHA256SUMS` with the desired artifact and run:

```console
sha256sum -c SHA256SUMS --ignore-missing
gh attestation verify <artifact> --repo gl-epka/sonicmux
```

Windows and macOS GUI packages in v0.1.0 are unsigned previews and can trigger
SmartScreen or Gatekeeper warnings. CLI/TUI archives require a system FFmpeg;
GUI bundles include the separately licensed FFmpeg build documented in
`THIRD_PARTY_LICENSES.md`.
EOF
