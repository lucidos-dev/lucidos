#!/usr/bin/env bash
#
# mirror-onnxruntime.sh: copy the ONNX Runtime archives a release build needs
# from upstream to the Lucidos mirror (ADR 0595). Run it on the maintainer's
# machine, once per ONNX Runtime version, before the release that needs it.
#
#   ./scripts/mirror-onnxruntime.sh            # dry run: download and verify only
#   ./scripts/mirror-onnxruntime.sh --upload   # also publish to the mirror repo
#
# Every archive is checked against the SHA-256 in the vendored dist.txt before
# upload, so the mirror only ever holds the bytes upstream published. The mirror
# is its own repo, one release per ONNX Runtime version, assets named as upstream.
# It never touches the Lucidos repo's releases (the desktop updater reads those).
# Verify afterwards with ./scripts/check-onnxruntime-mirror.sh.
set -Eeuo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/lib/release_draft.sh
source "$SCRIPT_DIR/lib/release_draft.sh"
# shellcheck source=scripts/lib/release_staging.sh
source "$SCRIPT_DIR/lib/release_staging.sh"
# shellcheck source=scripts/lib/onnxruntime_mirror.sh
source "$SCRIPT_DIR/lib/onnxruntime_mirror.sh"

publish=0
case "${1:-}" in
    "") ;;
    --upload) publish=1 ;;
    *) echo "usage: mirror-onnxruntime.sh [--upload]" >&2; exit 2 ;;
esac

repo="$(onnxruntime_mirror_repo)"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

rows="$(onnxruntime_mirror_rows)"
while IFS=$'\t' read -r triple upstream hash _ tag asset; do
    echo "==> $triple: downloading $asset"
    curl --fail --silent --show-error --location --retry 3 --retry-delay 2 \
        --connect-timeout 30 --max-time 600 -o "$work/$asset" "$upstream"
    [[ "$(release_staging_sha256 "$work/$asset")" == "$hash" ]] \
        || { echo "ERROR: $asset does not match dist.txt; refusing to mirror it." >&2; exit 1; }
    echo "    sha256 matches dist.txt"
    if [[ "$publish" == 1 ]]; then
        gh release view "$tag" --repo "$repo" >/dev/null 2>&1 \
            || gh release create "$tag" --repo "$repo" --title "ONNX Runtime $tag" \
                --notes "Static ONNX Runtime $tag archives, mirrored unchanged from upstream. See ADR 0595."
        gh release upload "$tag" "$work/$asset" --repo "$repo" --clobber
        echo "    uploaded to $repo $tag"
    fi
done <<< "$rows"

[[ "$publish" == 1 ]] || echo "Dry run: every archive verified. Re-run with --upload to publish to $repo."
