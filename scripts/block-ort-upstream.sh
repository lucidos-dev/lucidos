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

wipe_ort_state() {
    rm -rf "$(ort_cache_dir)/dfbin"
    rm -rf "$ROOT"/target/*/build/ort-sys-* "$ROOT"/target/*/.fingerprint/ort-sys-*
    echo "wiped the ort cache and ort-sys build artifacts"
}

assert_fetched_from_mirror() {
    local outputs=("$ROOT"/target/*/build/ort-sys-*/output)
    [[ -e "${outputs[0]}" ]] || { echo "ERROR: no ort-sys build output under $ROOT/target: it was not built." >&2; return 1; }
    grep -q "fetched ONNX Runtime from the Lucidos mirror" "${outputs[@]}" \
        || { echo "ERROR: ort-sys did not fetch from the Lucidos mirror. Output files: ${outputs[*]}" >&2; return 1; }
    echo "ort-sys fetched ONNX Runtime from the Lucidos mirror"
}

case "${1:-}" in
    "") block_host ;;
    --wipe) block_host; wipe_ort_state ;;
    --assert) assert_fetched_from_mirror ;;
    *) echo "usage: block-ort-upstream.sh [--wipe|--assert]" >&2; exit 2 ;;
esac
