#!/usr/bin/env bash
#
# check-onnxruntime-mirror.sh: prove the ONNX Runtime mirror holds every archive
# a release build needs, byte for byte (ADR 0595). Release preflight runs it
# before the tag is pushed, so a release never depends on an unpopulated mirror.
#
#   ./scripts/check-onnxruntime-mirror.sh
#
# Downloads each archive the vendored dist.txt names for a shipped triple from
# the mirror and compares its SHA-256 with the dist.txt row. Read-only. Exit 0
# when all match, 1 when any is missing or differs.
set -Eeuo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/lib/release_draft.sh
source "$SCRIPT_DIR/lib/release_draft.sh"
# shellcheck source=scripts/lib/release_staging.sh
source "$SCRIPT_DIR/lib/release_staging.sh"
# shellcheck source=scripts/lib/onnxruntime_mirror.sh
source "$SCRIPT_DIR/lib/onnxruntime_mirror.sh"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

bad=0
rows="$(onnxruntime_mirror_rows)"
while IFS=$'\t' read -r triple _ hash mirror_url _ asset; do
    if ! curl --fail --silent --show-error --location --retry 3 --retry-delay 2 \
        --connect-timeout 30 --max-time 600 -o "$work/$asset" "$mirror_url"; then
        echo "MISSING  $triple: $mirror_url" >&2
        bad=1
    elif [[ "$(release_staging_sha256 "$work/$asset")" != "$hash" ]]; then
        echo "DIFFERS  $triple: $mirror_url does not match dist.txt" >&2
        bad=1
    else
        echo "ok       $triple: $asset"
    fi
done <<< "$rows"

if [[ "$bad" != 0 ]]; then
    echo "The ONNX Runtime mirror is incomplete. Populate it with ./scripts/mirror-onnxruntime.sh --upload." >&2
    exit 1
fi
