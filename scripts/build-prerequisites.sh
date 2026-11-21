#!/usr/bin/env bash
# No compilation, installation or slot acquisition. The suite selector supplies
# desktop/headless explicitly, or passes Cargo selection flags after --.
# Never infer selection from installed libraries.
# --memory is the volatile recheck after slot admission; it ignores no receipt.
set -uo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/lib/build_prerequisites.sh
source "$SCRIPT_DIR/lib/build_prerequisites.sh"
# shellcheck source=scripts/lib/host_memory_guard.sh
source "$SCRIPT_DIR/lib/host_memory_guard.sh"
selection='' host='' target='' mode=all
cargo_selection=0 cargo_count=0
cargo_args=()
while [ $# -gt 0 ]; do
    case "$1" in
        --selection|--host|--target)
            [ $# -ge 2 ] || { echo "ERROR: $1 needs a value" >&2; exit 2; }
            case "$1" in
                --selection) selection=$2 ;;
                --host) host=$2 ;;
                --target) target=$2 ;;
            esac
            shift 2 ;;
        --memory) mode=memory; shift ;;
        --) shift; cargo_selection=1; cargo_count=$#; cargo_args=("$@"); break ;;
        *) echo "ERROR: unknown prerequisite argument: $1" >&2; exit 2 ;;
    esac
done
if [ -z "$host" ] || [ -z "$target" ] || { [ -z "$selection" ] && [ "$cargo_selection" -eq 0 ]; }; then
    echo 'Usage: build-prerequisites.sh --host TRIPLE --target TRIPLE [--memory] ( --selection desktop|headless | -- CARGO_SELECTION_FLAGS )' >&2
    exit 2
fi
if [ "$cargo_selection" -eq 1 ]; then
    if [ -n "$selection" ] || [ "$mode" = memory ]; then
        echo 'ERROR: Cargo selection cannot be combined with --selection or --memory' >&2
        exit 2
    fi
    if [ "$cargo_count" -gt 0 ]; then
        selection="$(bp_cargo_selection "$target" "${cargo_args[@]}")" || exit 1
    else
        selection="$(bp_cargo_selection "$target")" || exit 1
    fi
fi
bp_build_check "$(uname -s)" "$host" "$target" "$selection" "$mode" "$(host_memory_build_line_gb)"
