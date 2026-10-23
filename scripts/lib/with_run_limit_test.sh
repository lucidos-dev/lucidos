#!/bin/bash
# Tests for scripts/with-run-limit.sh (ADR 0391): a command inside its limit
# keeps its own exit status, and one past it is stopped with its whole tree.
#
# Hermetic: every process signalled is one this suite spawned, found by
# walking the tree down from the wrapper's own child (ADR 0025).
#
# Run: ./scripts/lib/with_run_limit_test.sh
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
RUN_LIMIT="$SCRIPT_DIR/../with-run-limit.sh"

PASS=0
FAIL=0
pass() { echo "  ok:   $*"; PASS=$((PASS + 1)); }
fail() { echo "  FAIL: $*"; FAIL=$((FAIL + 1)); }
expect_eq() { # <name> <expected> <actual>
    if [ "$2" = "$3" ]; then pass "$1"; else fail "$1: expected [$2], got [$3]"; fi
}
gone() { # <pid>
    ! kill -0 "$1" 2>/dev/null
}

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
export LUCIDOS_RUN_LIMIT_GRACE_SECS=1

echo "a command inside its limit keeps its exit status"
"$RUN_LIMIT" 30 "quick" -- bash -c 'exit 3' 2>/dev/null
expect_eq "exit status passes through" 3 "$?"

echo "a command past its limit is stopped with its tree"
"$RUN_LIMIT" 1 "hung test" -- bash -c "sleep 300 & echo \$! > '$TMP/grandchild'; wait" 2> "$TMP/stderr"
expect_eq "the limit exits 124" 124 "$?"
if grep -q "hung test ran past its 1s limit" "$TMP/stderr"; then
    pass "the message names the run and its limit"
else
    fail "the message names the run and its limit: $(cat "$TMP/stderr")"
fi
if gone "$(cat "$TMP/grandchild")"; then
    pass "the grandchild is gone"
else
    fail "the grandchild is gone"
    kill -KILL "$(cat "$TMP/grandchild")" 2>/dev/null
fi

echo "a tree that ignores SIGTERM still ends"
"$RUN_LIMIT" 1 "stubborn" -- bash -c "trap '' TERM; sleep 300 & echo \$! > '$TMP/stubborn'; wait" 2>/dev/null
expect_eq "the limit exits 124" 124 "$?"
if gone "$(cat "$TMP/stubborn")"; then
    pass "SIGKILL reached what ignored SIGTERM"
else
    fail "SIGKILL reached what ignored SIGTERM"
    kill -KILL "$(cat "$TMP/stubborn")" 2>/dev/null
fi

echo "bad usage is refused"
"$RUN_LIMIT" soon "label" -- true 2>/dev/null
expect_eq "a non-numeric limit exits 2" 2 "$?"
"$RUN_LIMIT" 5 "label" true 2>/dev/null
expect_eq "a missing -- exits 2" 2 "$?"

echo
echo "with_run_limit_test: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
