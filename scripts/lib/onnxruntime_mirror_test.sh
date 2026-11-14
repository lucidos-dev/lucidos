#!/usr/bin/env bash
# Tests for scripts/lib/onnxruntime_mirror.sh: which archives the ONNX Runtime
# mirror must hold (ADR 0595). Offline: reads only the vendored ort-sys files.
# Run: ./scripts/lib/onnxruntime_mirror_test.sh
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/lib/release_draft.sh
source "$SCRIPT_DIR/release_draft.sh"
# shellcheck source=scripts/lib/onnxruntime_mirror.sh
source "$SCRIPT_DIR/onnxruntime_mirror.sh"

PASS=0
FAIL=0
fail() { echo "  FAIL: $*"; FAIL=$((FAIL+1)); }
pass() { echo "  ok:   $*"; PASS=$((PASS+1)); }

rows="$(onnxruntime_mirror_rows)"

if [ "$(printf '%s\n' "$rows" | wc -l | tr -d ' ')" = "$(release_draft_triples | wc -l | tr -d ' ')" ]; then
    pass "one row per shipped triple"
else
    fail "row count differs from the shipped triple count"
fi

while IFS=$'\t' read -r triple upstream hash mirror_url tag asset; do
    case "$mirror_url" in
        "$(onnxruntime_mirror_base_url)/$tag/$asset") pass "$triple: mirror URL is base, tag, asset" ;;
        *) fail "$triple: unexpected mirror URL $mirror_url" ;;
    esac
    case "$upstream" in
        *"/msort-binary/$tag/$asset") pass "$triple: upstream path carries the same tag and asset" ;;
        *) fail "$triple: upstream URL $upstream does not end in $tag/$asset" ;;
    esac
    if printf '%s' "$hash" | grep -Eq '^[0-9a-f]{64}$'; then pass "$triple: hash is 64 lowercase hex"; else fail "$triple: bad hash '$hash'"; fi
done <<< "$rows"

if [ "$(onnxruntime_mirror_repo)" = "lucidos-dev/onnxruntime-mirror" ]; then
    pass "mirror repo is derived from the base URL in build.rs"
else
    fail "mirror repo is '$(onnxruntime_mirror_repo)'"
fi

# A base URL build.rs no longer carries must fail loudly, never yield zero rows.
empty="$(mktemp -d)"
cp "$ONNXRUNTIME_MIRROR_SYS_DIR/dist.txt" "$empty/dist.txt"
: > "$empty/build.rs"
if ONNXRUNTIME_MIRROR_SYS_DIR="$empty" onnxruntime_mirror_rows >/dev/null 2>&1; then
    fail "rows succeeded with no MIRROR_BASE_URL"
else
    pass "rows fail when build.rs has no MIRROR_BASE_URL"
fi
# Compile the std-only retry module used by build.rs, then run every regression.
if rustc --edition=2021 --test "$ONNXRUNTIME_MIRROR_SYS_DIR/src/internal/download.rs" -o "$empty/download-tests" \
    && "$empty/download-tests"; then
    pass "build-script download retry regressions"
else
    fail "build-script download retry regressions"
fi
rm -rf "$empty"

echo "passed $PASS, failed $FAIL"
[ "$FAIL" = 0 ]
