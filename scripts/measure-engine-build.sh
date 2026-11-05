#!/usr/bin/env bash
# measure-engine-build.sh: the benchmark behind the engine crate split's gate.
#
# Times what a coding agent pays for an engine change, so each split phase can
# prove its win (ADR 0392, docs/plans/2026-10-08-engine-crate-split-and-cold-builds.md).
#
# Usage:
#   scripts/measure-engine-build.sh [--cold] [--packages "<pkg> ..."]
#                                   [--out <dir>] [--compare <results.tsv>]
#
#   --cold       Also time cold builds in a fresh temporary target dir.
#   --packages   The engine packages to build and test. Default: lucidos-engine.
#                A split phase lists every tier crate here.
#   --out        Where results.tsv and the cargo timing reports go.
#   --compare    A results.tsv from an earlier run. Prints the change per step.
#
# Each probe appends one private function to a file, then times an
# incremental build, test build and `make lint`'s clippy pair. Probe files must
# be clean at the start, and the script restores them on exit.
#
# Every cargo step takes its own build slot, and the steps run one at a time:
# two cargo runs in one worktree can exhaust the host's memory.

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
cd "$PROJECT_DIR" || exit 1

# File tails, not paths, so a probe follows its module across a crate move.
PROBE_TAILS=(
    "src/engine/agent_session/mod.rs"
    "src/engine/chat/mod.rs"
    "src/engine/tools/mod.rs"
    "src/llm/mod.rs"
    "src/api/threads/mod.rs"
)

COLD=0
PACKAGES="lucidos-engine"
OUT_DIR=""
COMPARE=""
while [ $# -gt 0 ]; do
    case "$1" in
        --cold) COLD=1; shift ;;
        --packages) PACKAGES="${2:?--packages needs a value}"; shift 2 ;;
        --out) OUT_DIR="${2:?--out needs a value}"; shift 2 ;;
        --compare) COMPARE="${2:?--compare needs a value}"; shift 2 ;;
        -h | --help) sed -n '2,22p' "$0"; exit 0 ;;
        *) echo "measure-engine-build.sh: unknown argument: $1" >&2; exit 2 ;;
    esac
done

[ -n "$OUT_DIR" ] || OUT_DIR="$(mktemp -d "${TMPDIR:-/tmp}/measure-engine-build.XXXXXX")"
mkdir -p "$OUT_DIR"
RESULTS="$OUT_DIR/results.tsv"
: > "$RESULTS"

PKG_ARGS=()
for pkg in $PACKAGES; do PKG_ARGS+=(-p "$pkg"); done

PROBE_FILES=()
for tail in "${PROBE_TAILS[@]}"; do
    match="$(git ls-files "crates/*/$tail" | head -1)"
    if [ -z "$match" ]; then
        echo "measure-engine-build.sh: no tracked file ends in $tail" >&2
        exit 2
    fi
    PROBE_FILES+=("$match")
done

if ! git diff --quiet -- "${PROBE_FILES[@]}"; then
    echo "measure-engine-build.sh: a probe file has uncommitted changes:" >&2
    git diff --name-only -- "${PROBE_FILES[@]}" >&2
    exit 2
fi

COLD_TARGET=""
# A cold target holds gigabytes, so a failed step must not leave it behind.
clean_up() {
    git checkout -- "${PROBE_FILES[@]}"
    [ -z "$COLD_TARGET" ] || rm -rf "$COLD_TARGET"
}
trap clean_up EXIT

PROBE_SEQ=0
# Insert above the first `#[cfg(test)]`, since clippy's items_after_test_module
# refuses an item below a test module. A file without one gets it appended.
probe() { # <file>
    PROBE_SEQ=$((PROBE_SEQ + 1))
    awk -v n="$PROBE_SEQ" '
        function emit() {
            printf "#[allow(dead_code)]\nfn measure_engine_build_probe_%s() -> u32 {\n    %s\n}\n\n", n, n
            done = 1
        }
        !done && /^#\[cfg\(test\)\]/ { emit() }
        { print }
        END { if (!done) { print ""; emit() } }
    ' "$1" > "$1.probe" && mv "$1.probe" "$1"
}

# Time one cargo command inside a build slot, excluding the wait for the slot.
step() { # <name> <command...>
    local name="$1"; shift
    local log="$OUT_DIR/$name.log"
    local report="${CARGO_TARGET_DIR:-target}/cargo-timings/cargo-timing.html"
    rm -f "$report"
    "$SCRIPT_DIR/with-build-slot.sh" --label "measure-engine-build: $name" -- \
        bash -c 'TIMEFORMAT="MEASURE_SECONDS %R"; time "$@"' _ "$@" > "$log" 2>&1
    local rc=$?
    local secs
    secs="$(grep '^MEASURE_SECONDS ' "$log" | tail -1 | awk '{print $2}')"
    if [ "$rc" -ne 0 ]; then
        echo "measure-engine-build.sh: step $name failed (exit $rc), see $log" >&2
        exit "$rc"
    fi
    [ -f "$report" ] && cp "$report" "$OUT_DIR/$name.timing.html"
    # The slot's share of the host. Two runs compare fairly only at equal cores.
    local cores
    cores="$(grep -o -E '[0-9]+ cores$' "$log" | head -1 | awk '{print $1}')"
    printf '%s\t%s\t%s\n' "$name" "$secs" "${cores:-?}" >> "$RESULTS"
    printf '%-34s %8ss  %3s cores\n' "$name" "$secs" "${cores:-?}"
}

run_build() { step "$1" cargo build --locked "${PKG_ARGS[@]}" --timings; }
run_test() { step "$1" cargo test --locked "${PKG_ARGS[@]}" --lib --no-run --timings; }
run_clippy() { step "$1" make --no-print-directory lint-rust-clippy; }

echo "packages: $PACKAGES"
echo "results:  $RESULTS"

if [ "$COLD" -eq 1 ]; then
    COLD_TARGET="$(mktemp -d "${TMPDIR:-/tmp}/measure-engine-build-target.XXXXXX")"
    CARGO_TARGET_DIR="$COLD_TARGET" run_build cold_build
    CARGO_TARGET_DIR="$COLD_TARGET" run_test cold_test
    CARGO_TARGET_DIR="$COLD_TARGET" run_clippy cold_clippy
    rm -rf "$COLD_TARGET"
    COLD_TARGET=""
fi

# Warm the checkout's own target so each probe below measures one change.
run_build warm_build
run_test warm_test
run_clippy warm_clippy
run_build noop_build

for file in "${PROBE_FILES[@]}"; do
    module="$(basename "$(dirname "$file")")"
    probe "$file"; run_build "edit_${module}_build"
    probe "$file"; run_test "edit_${module}_test"
    probe "$file"; run_clippy "edit_${module}_clippy"
done

if [ -n "$COMPARE" ]; then
    echo
    echo "change against $COMPARE:"
    awk -F'\t' 'NR == FNR { before[$1] = $2; cores[$1] = $3; next }
        ($1 in before) && before[$1] > 0 {
            note = (cores[$1] == $3) ? "" : sprintf("  (cores %s -> %s, not comparable)", cores[$1], $3)
            printf "%-34s %8.1fs -> %8.1fs  %+6.1f%%%s\n", $1, before[$1], $2, 100 * ($2 - before[$1]) / before[$1], note
        }' "$COMPARE" "$RESULTS"
fi
