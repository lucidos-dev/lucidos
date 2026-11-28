#!/bin/bash
# The engine suite's run limit guards the test run, never the compile.
#
# A SOURCE SCAN of scripts/test-engine.sh. A cold macOS runner compiled for 63
# minutes against a usual 22 to 29, so a limit that wrapped compile and tests
# together stopped a healthy suite as if a test had hung. The fix is an ordering:
# a `--no-run` build outside the limit, then the limited run. Each check runs
# over the real script and over a copy doctored back to the old shape.
#
# Run: ./scripts/lib/test_engine_build_outside_limit_test.sh
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
TARGET="$SCRIPT_DIR/../test-engine.sh"

SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT

PASS=0
FAIL=0
pass() { echo "  ok:   $*"; PASS=$((PASS + 1)); }
fail() { echo "  FAIL: $*"; FAIL=$((FAIL + 1)); }

# One problem per line, nothing when the script is correct.
scan() { # <script>
    local script="$1" build run
    build="$(grep -nE '^[[:space:]]+cargo test --locked --no-run ' "$script" | head -1 | cut -d: -f1)"
    # shellcheck disable=SC2016 # a grep pattern: the dollar signs are the text matched
    run="$(grep -nE '^"\$SLOT" --label "engine tests" -- "\$\{LIMIT\[@\]\}"' "$script" | head -1 | cut -d: -f1)"
    if [ -z "$build" ]; then
        echo "no --no-run build, so compile time counts against the run limit"
    elif [ -z "$run" ]; then
        echo "the limited test run is gone, so nothing here can be checked"
    elif [ "$build" -ge "$run" ]; then
        echo "the --no-run build comes after the limited run"
    fi
}

echo "test: the real script builds outside the limit, then runs under it"
problems="$(scan "$TARGET")"
if [ -z "$problems" ]; then pass "ordering holds"; else fail "$problems"; fi

echo "test: a script that compiles inside the limit is caught"
grep -v 'cargo test --locked --no-run' "$TARGET" >"$SCRATCH/old.sh"
problems="$(scan "$SCRATCH/old.sh")"
if echo "$problems" | grep -q "no --no-run build"; then pass "caught"; else fail "not caught: $problems"; fi

echo "test_engine_build_outside_limit_test.sh: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
