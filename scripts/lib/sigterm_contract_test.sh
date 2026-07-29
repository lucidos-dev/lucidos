#!/bin/bash
# Pins the SIGTERM-survival contract across the engine + scripts:
#   * Engine main.rs installs a SIGTERM ignorer (catches and logs, never acts)
#     and accepts SIGUSR1 as the legitimate stop signal.
#   * scripts/lib/workspace.sh kill_stale_processes sends SIGUSR1 (not SIGTERM)
#     to the engine pid, so /api/v1/restart still works after the engine starts
#     ignoring SIGTERM.
#   * scripts/stop.sh sends SIGUSR1 (not SIGTERM) to the engine pid, so the
#     user-facing stop command still works.
#
# Without these all aligned, either accidental kills from CC subprocess test
# scripts still take down the engine (incident: thread 29da40d1 ran
# `lsof -ti :5173 | xargs kill` inside a test, killed the engine, scope was a
# port-selection feature), or legitimate restart/stop paths break silently.
#
# Run: ./scripts/lib/sigterm_contract_test.sh
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$(dirname "$SCRIPT_DIR")")"

PASS=0
FAIL=0
fail() { echo "  FAIL: $*"; FAIL=$((FAIL+1)); }
pass() { echo "  ok:   $*"; PASS=$((PASS+1)); }

MAIN_RS="$PROJECT_DIR/crates/lucidos-engine/src/main.rs"

echo "test: main.rs installs SIGUSR1 handler as legitimate stop signal"
if grep -q "SignalKind::user_defined1()" "$MAIN_RS"; then
    pass "SignalKind::user_defined1() present in main.rs"
else
    fail "main.rs does not install a SIGUSR1 handler"
fi

echo "test: main.rs installs a SIGTERM ignorer (catches and logs, never acts)"
if grep -q "SignalKind::terminate()" "$MAIN_RS" \
   && grep -q "Received SIGTERM" "$MAIN_RS"; then
    pass "SIGTERM ignorer + log line present in main.rs"
else
    fail "main.rs does not install + log-ignore SIGTERM"
fi

echo "test: workspace.sh kill_stale_processes uses kill -USR1 for engine"
WORKSPACE_SH="$SCRIPT_DIR/workspace.sh"
# The engine-only kills sit inside the `if [ -n "$BUILD" ] ...; then ... fi`
# guard within kill_stale_processes. The condition also carries a
# `&& [ -z "$skip_engine_kill" ]` clause, so match the `if [ -n "$BUILD" ]`
# PREFIX, not the whole line. The same function later kills the FRONTEND / legacy
# build-watch with plain SIGTERM — those sit AFTER this block's 4-space `fi`, so
# scoping the grep to the block (until that `fi`) correctly excludes them.
BUILD_BLOCK=$(awk '
    /^kill_stale_processes\(\)/{in_func=1}
    in_func && /^    if \[ -n "\$BUILD" \]/{f=1}
    f{print}
    f && /^    fi$/{exit}
' "$WORKSPACE_SH")
ENGINE_KILLS=$(echo "$BUILD_BLOCK" | grep -E '^\s+kill\s+(-[A-Z0-9]+\s+)?"\$')
if [ -z "$ENGINE_KILLS" ]; then
    fail "could not locate engine kill calls in BUILD block — refactored?"
elif echo "$ENGINE_KILLS" | grep -vq -- "-USR1"; then
    fail "kill_stale_processes BUILD block still has non-USR1 engine kills:"
    # shellcheck disable=SC2001 # per-line indent of a multi-line block; ${//} cannot anchor to ^
    echo "$ENGINE_KILLS" | sed 's/^/      /' >&2
else
    pass "all BUILD-block engine kills use -USR1"
fi

echo "test: stop.sh stops engine via kill -USR1 (frontend kill stays SIGTERM)"
STOP_SH="$PROJECT_DIR/scripts/stop.sh"
# The engine kill is the one inside the `if [ -f "$engine_pid_file" ]; then` block.
# Scope to that block; the frontend kill block follows and stays SIGTERM.
ENGINE_BLOCK=$(awk '
    /if \[ -f "\$engine_pid_file" \]; then/{f=1}
    f{print}
    f && /^    fi$/{exit}
' "$STOP_SH")
if [ -z "$ENGINE_BLOCK" ]; then
    fail "could not locate engine pid block in stop.sh — refactored?"
elif ! echo "$ENGINE_BLOCK" | grep -q "kill -USR1"; then
    fail "stop.sh engine block does not use kill -USR1:"
    # shellcheck disable=SC2001 # per-line indent of a multi-line block; ${//} cannot anchor to ^
    echo "$ENGINE_BLOCK" | sed 's/^/      /' >&2
else
    pass "stop.sh uses kill -USR1 for engine"
fi

echo ""
echo "Passed: $PASS  Failed: $FAIL"
[ $FAIL -eq 0 ]
