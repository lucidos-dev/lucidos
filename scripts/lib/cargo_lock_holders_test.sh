#!/bin/bash
# Tests for select_cargo_lock_holders in scripts/lib/workspace.sh.
#
# Regression context: build_or_find_engine clears IDE / rust-analyzer
# `cargo check` processes that hold the SHARED target/ build lock before a
# rebuild. It used a raw `pgrep -f 'cargo check' | xargs kill`, which
# substring-matches the phrase ANYWHERE in a process's command line — so it
# also killed coding-agent subprocesses (claude / codex) whose injected
# prompt merely CONTAINED "cargo check" (a CC session working on a build
# does). Killing those by PID bypasses their process-group isolation and
# SIGTERMs a live CC session in THIS workspace or, because target/ is shared
# across workspaces from one checkout, in ANOTHER workspace entirely — the
# exit=143 cross-workspace kill that silently terminated a parked session
# during an unrelated workspace's rebuild.
#
# select_cargo_lock_holders filters pgrep's matches down to processes whose
# argv[0] basename is `cargo`, so a CC subprocess is never targeted.
#
# Run: ./scripts/lib/cargo_lock_holders_test.sh
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PASS=0
FAIL=0
fail() { echo "  FAIL: $*"; FAIL=$((FAIL+1)); }
pass() { echo "  ok:   $*"; PASS=$((PASS+1)); }

# shellcheck source=workspace.sh
source "$SCRIPT_DIR/workspace.sh"

SPAWNED=""
cleanup() {
    # shellcheck disable=SC2086
    [ -n "$SPAWNED" ] && kill $SPAWNED 2>/dev/null
    wait 2>/dev/null
}
trap cleanup EXIT

# A fake coding-agent subprocess: argv[0] is `claude` and "cargo check"
# appears LATER in the command line — exactly how a CC subprocess carries
# the phrase in an injected prompt / allowed-tool. Must NEVER be selected.
# `yes` is a real, signed binary (copies of system binaries don't exec under
# SIP), and `exec -a` lets us control argv[0]; output is discarded.
( exec -a "claude" yes "the worktree could only cargo check, never bundle" >/dev/null 2>&1 ) &
FAKE_CC_PID=$!
SPAWNED="$SPAWNED $FAKE_CC_PID"

# A fake real cargo lock-holder: argv[0] is `cargo`, with `check` as a later
# token (as rust-analyzer runs it). MUST be selected.
( exec -a "cargo" yes check --workspace --message-format=json >/dev/null 2>&1 ) &
FAKE_CARGO_PID=$!
SPAWNED="$SPAWNED $FAKE_CARGO_PID"

# Let the execs land so pgrep/ps observe the final argv.
sleep 0.5

holders="$(select_cargo_lock_holders)"

echo "test: real cargo lock-holder IS selected"
if printf '%s\n' "$holders" | grep -qx "$FAKE_CARGO_PID"; then
    pass "cargo process $FAKE_CARGO_PID selected"
else
    fail "cargo process $FAKE_CARGO_PID NOT selected (holders='$holders')"
fi

echo "test: coding-agent subprocess with 'cargo check' in argv is NOT selected"
if printf '%s\n' "$holders" | grep -qx "$FAKE_CC_PID"; then
    fail "CC-like process $FAKE_CC_PID selected — would SIGTERM a live session (the cross-workspace kill)"
else
    pass "CC-like process $FAKE_CC_PID correctly skipped"
fi

echo ""
echo "select_cargo_lock_holders: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
