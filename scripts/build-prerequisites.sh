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
cargo_selection=0
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
        --)
            shift
            cargo_selection=1
            while [ $# -gt 0 ]; do
                case "$1" in
                    --target)
                        [ $# -ge 2 ] || { echo 'ERROR: --target needs a value' >&2; exit 2; }
                        target=$2
                        shift ;;
                    --target=*) target=${1#--target=} ;;
                    *) cargo_args+=("$1") ;;
                esac
                shift
            done ;;
        *) echo "ERROR: unknown prerequisite argument: $1" >&2; exit 2 ;;
    esac
done
if [ "$mode" = memory ] && [ "$cargo_selection" -eq 0 ] && [ -z "$selection$host$target" ]; then
    bp_build_check "$(uname -s)" "" "" headless memory "$(host_memory_build_line_gb)"
    exit $?
fi
if [ -z "$selection" ] && [ "$cargo_selection" -eq 0 ]; then
    echo 'Usage: build-prerequisites.sh --memory, or build-prerequisites.sh [--host TRIPLE] [--target TRIPLE] ( --selection desktop|headless | -- CARGO_SELECTION_FLAGS )' >&2
    exit 2
fi
if [ -z "$host" ]; then
    host="$("${RUSTC:-rustc}" -vV | awk '$1 == "host:" {print $2}')" || exit 1
    if [ -z "$host" ]; then
        echo 'ERROR: cannot read the Rust host target from rustc -vV' >&2
        exit 1
    fi
fi
# Cargo accepts build.target from ancestor and user config. Require an
# explicit override before certifying a target we cannot identify from flags.
if [ -z "$target" ] && [ -z "${CARGO_BUILD_TARGET:-}" ]; then
    cargo_home=${CARGO_HOME:-$HOME/.cargo}
    directory=$PWD
    while :; do
        for config in "$directory/.cargo/config" "$directory/.cargo/config.toml" "$cargo_home/config" "$cargo_home/config.toml"; do
            [ -f "$config" ] || continue
            if grep -Eq '^[[:space:]]*((build[[:space:]]*\.[[:space:]]*)?target[[:space:]]*=|build[[:space:]]*=[[:space:]]*\{.*target[[:space:]]*=)' "$config"; then
                echo "ERROR: set CARGO_BUILD_TARGET explicitly for build preflight ($config)" >&2
                exit 1
            fi
        done
        [ "$directory" != / ] || break
        directory="$(dirname "$directory")"
    done
fi
target=${target:-${CARGO_BUILD_TARGET:-$host}}
if [ "$cargo_selection" -eq 1 ]; then
    if [ -n "$selection" ] || [ "$mode" = memory ]; then
        echo 'ERROR: Cargo selection cannot be combined with --selection or --memory' >&2
        exit 2
    fi
    case "$target" in
        *-linux-*)
            if ! selection="$(bp_cargo_selection "$target" ${cargo_args[@]+"${cargo_args[@]}"})"; then
                bp_build_check "$(uname -s)" "$host" "$target" headless memory "$(host_memory_build_line_gb)"
                exit 1
            fi ;;
        *) selection=headless ;;
    esac
fi
bp_build_check "$(uname -s)" "$host" "$target" "$selection" "$mode" "$(host_memory_build_line_gb)"
