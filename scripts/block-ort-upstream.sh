#!/usr/bin/env bash
#
# block-ort-upstream.sh: prove a CI build needs only the ONNX Runtime mirror
# (ADR 0595). The vendored ort-sys fetches from the mirror before
# parcel.pyke.io, and only a build with pyke unreachable shows that the mirror
# alone suffices.
#
#   block-ort-upstream.sh [--wipe]   point parcel.pyke.io at 0.0.0.0; --wipe also
#                                    clears the ort cache and ort-sys's build
#                                    artifacts so the build must fetch again
#   block-ort-upstream.sh --assert   fail unless the last build fetched ONNX
#                                    Runtime from the mirror
#
# Run it for CI only: it edits /etc/hosts. A rust-cache hit restores ort-sys's
# build output, so a build that fetches nothing would pass vacuously. The
# --assert mode turns that vacuous pass into a failure.
set -Eeuo pipefail

HOST="parcel.pyke.io"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

block_host() {
    local sudo=""
    [[ "$(id -u)" == 0 ]] || sudo="sudo"
    # Both families: macOS still asks DNS for AAAA when only an IPv4 entry exists.
    printf '0.0.0.0 %s\n:: %s\n' "$HOST" "$HOST" | $sudo tee -a /etc/hosts >/dev/null
    echo "blocked $HOST in /etc/hosts"
}

# Mirrors cache_dir() in ort-sys's src/internal/dirs.rs, which build.rs joins
# with "dfbin". On Linux an absolute XDG_CACHE_HOME is used as is, without PYKE_ROOT.
ort_cache_dir() {
    if [[ "$(uname -s)" == Darwin ]]; then
        echo "$HOME/Library/Caches/ort.pyke.io"
    elif [[ "${XDG_CACHE_HOME:-}" == /* ]]; then
        echo "$XDG_CACHE_HOME"
    else
        echo "$HOME/.cache/ort.pyke.io"
    fi
}

# Only known build roots, not arbitrary nested caches. Cargo can place profiles
# directly under the target root or under a cross-compilation triple.
ort_build_dirs() {
    local roots=("$ROOT/target" "$ROOT/target/launch-tree/target")
    local root dir
    if [[ -n "${CARGO_TARGET_DIR:-}" ]]; then
        roots+=("$CARGO_TARGET_DIR")
    fi
    for root in "${roots[@]}"; do
        for dir in "$root"/*/build "$root"/*/*/build; do
            [[ -d "$dir" ]] && printf '%s\n' "$dir"
        done
    done
    return 0
}

wipe_ort_state() {
    local dir
    rm -rf "$(ort_cache_dir)/dfbin"
    while IFS= read -r dir; do
        rm -rf "$dir"/ort-sys-* "${dir%/build}"/.fingerprint/ort-sys-*
    done < <(ort_build_dirs)
    echo "wiped the ort cache and ort-sys build artifacts"
}

assert_fetched_from_mirror() {
    local dir output count=0
    while IFS= read -r dir; do
        for output in "$dir"/ort-sys-*/output; do
            [[ -f "$output" ]] || continue
            count=$((count + 1))
            if ! grep -qF "fetched ONNX Runtime from the Lucidos mirror:" "$output" \
                || grep -qF "fetched ONNX Runtime from upstream:" "$output"; then
                echo "ERROR: ort-sys lacks exclusive mirror-fetch evidence: $output" >&2
                return 1
            fi
        done
    done < <(ort_build_dirs)
    [[ "$count" -gt 0 ]] || { echo "ERROR: no ort-sys build output in known target roots: it was not built." >&2; return 1; }
    echo "ort-sys fetched ONNX Runtime from the Lucidos mirror"
}

case "${1:-}" in
    "") block_host ;;
    --wipe) block_host; wipe_ort_state ;;
    --assert) assert_fetched_from_mirror ;;
    *) echo "usage: block-ort-upstream.sh [--wipe|--assert]" >&2; exit 2 ;;
esac
