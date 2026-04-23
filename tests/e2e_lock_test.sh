#!/bin/bash
# Tests the e2e workspace lock that prevents concurrent CC sessions from
# clobbering each other on ~/workspaces/e2e-test (Playwright multiplies WebKit
# GPU processes — two parallel runs on a 32 GB Mac OOM'd the system on
# 2026-04-19).
#
# Run: ./tests/e2e_lock_test.sh
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"

# Isolate test runs from any real e2e workspace lock
TMPROOT="$(mktemp -d -t cognos-e2e-lock-test.XXXXXX)"
export E2E_LOCK_DIR_OVERRIDE="$TMPROOT/lock-dir"
mkdir -p "$E2E_LOCK_DIR_OVERRIDE"
OUT_DIR="$TMPROOT/out"
mkdir -p "$OUT_DIR"
trap 'rm -rf "$TMPROOT"' EXIT

source "$PROJECT_DIR/scripts/lib/e2e_lock.sh"

PASS=0
FAIL=0

assert_eq() {
    local expected="$1" actual="$2" msg="$3"
    if [ "$expected" = "$actual" ]; then
        echo "  PASS: $msg"
        PASS=$((PASS + 1))
    else
        echo "  FAIL: $msg (expected '$expected', got '$actual')"
        FAIL=$((FAIL + 1))
    fi
}

reset_lock_dir() {
    rm -rf "$E2E_LOCK_DIR_OVERRIDE"
    mkdir -p "$E2E_LOCK_DIR_OVERRIDE"
}

# ── 1. First acquire succeeds ────────────────────────────────────────────
echo "Test 1: first acquire succeeds"
reset_lock_dir
acquire_e2e_lock e2e-browser >"$OUT_DIR"/test-1.out 2>&1
assert_eq "0" "$?" "first acquire returns 0"
assert_eq "1" "$([ -f "$E2E_LOCK_DIR_OVERRIDE/e2e.lock" ] && echo 1 || echo 0)" "lock file created"
release_e2e_lock

# ── 2. Second concurrent acquire fails ───────────────────────────────────
echo "Test 2: concurrent acquire blocked"
reset_lock_dir
acquire_e2e_lock e2e-browser >"$OUT_DIR"/test-2a.out 2>&1
first_rc=$?
acquire_e2e_lock e2e-api >"$OUT_DIR"/test-2b.out 2>&1
second_rc=$?
assert_eq "0" "$first_rc" "first acquire returns 0"
assert_eq "1" "$second_rc" "second acquire returns 1"
if grep -q "another e2e run is in progress" "$OUT_DIR"/test-2b.out; then
    echo "  PASS: error message names the conflict"
    PASS=$((PASS + 1))
else
    echo "  FAIL: error message missing 'another e2e run is in progress'"
    echo "  ---"
    cat "$OUT_DIR"/test-2b.out
    echo "  ---"
    FAIL=$((FAIL + 1))
fi
if grep -qE "PID [0-9]+" "$OUT_DIR"/test-2b.out; then
    echo "  PASS: error message includes owning PID"
    PASS=$((PASS + 1))
else
    echo "  FAIL: error message missing owning PID"
    FAIL=$((FAIL + 1))
fi
release_e2e_lock

# ── 3. Stale lock (dead PID) is reclaimed ────────────────────────────────
echo "Test 3: stale lock reclaimed"
reset_lock_dir
# 999999 is virtually guaranteed to not exist (macOS PID_MAX is 99999)
cat > "$E2E_LOCK_DIR_OVERRIDE/e2e.lock" <<EOF
PID=999999
THREAD_ID=ghost
WORKTREE=/tmp/ghost
STARTED=2020-01-01T00:00:00Z
SCRIPT=e2e-browser
EOF
acquire_e2e_lock e2e-browser >"$OUT_DIR"/test-3.out 2>&1
assert_eq "0" "$?" "stale lock reclaimed (acquire returns 0)"
release_e2e_lock

# ── 4. Release only removes our own lock ─────────────────────────────────
echo "Test 4: release does not remove other-owner lock"
reset_lock_dir
cat > "$E2E_LOCK_DIR_OVERRIDE/e2e.lock" <<EOF
PID=$$
THREAD_ID=other
WORKTREE=/tmp/other
STARTED=2020-01-01T00:00:00Z
SCRIPT=e2e-api
EOF
# Pretend we never acquired (E2E_LOCK_OWNED is unset/empty)
unset E2E_LOCK_OWNED
release_e2e_lock
assert_eq "1" "$([ -f "$E2E_LOCK_DIR_OVERRIDE/e2e.lock" ] && echo 1 || echo 0)" "other-owner lock preserved"

# ── 5. Release removes our own lock ──────────────────────────────────────
echo "Test 5: release removes our own lock"
reset_lock_dir
acquire_e2e_lock e2e-browser >/dev/null 2>&1
release_e2e_lock
assert_eq "0" "$([ -f "$E2E_LOCK_DIR_OVERRIDE/e2e.lock" ] && echo 1 || echo 0)" "own lock removed"

# ── 6. Entry-point script bails out when lock is held ────────────────────
# Simulates the original incident: hold the lock from a "first" CC session,
# then invoke e2e-browser.sh as the "second" session and verify it exits
# before touching the workspace.
echo "Test 6: e2e-browser.sh bails out when lock held"
reset_lock_dir
# Plant a held lock owned by the test process itself (PID is alive)
cat > "$E2E_LOCK_DIR_OVERRIDE/e2e.lock" <<EOF
PID=$$
THREAD_ID=test-holder
WORKTREE=/tmp/holder
STARTED=$(date -u +%Y-%m-%dT%H:%M:%SZ)
SCRIPT=e2e-browser
EOF
"$PROJECT_DIR/scripts/e2e-browser.sh" -f none.spec.ts >"$OUT_DIR"/test-6.out 2>&1
script_rc=$?
assert_eq "1" "$script_rc" "e2e-browser.sh exits 1"
if grep -q "another e2e run is in progress" "$OUT_DIR"/test-6.out; then
    echo "  PASS: e2e-browser.sh prints conflict message"
    PASS=$((PASS + 1))
else
    echo "  FAIL: e2e-browser.sh missing conflict message"
    echo "  ---"
    cat "$OUT_DIR"/test-6.out
    echo "  ---"
    FAIL=$((FAIL + 1))
fi
# Verify the script did NOT start the workspace (no engine PID written, no DB reset)
if ! grep -qE "Starting e2e workspace|Engine already running|Resetting database" "$OUT_DIR"/test-6.out; then
    echo "  PASS: e2e-browser.sh aborted before workspace startup"
    PASS=$((PASS + 1))
else
    echo "  FAIL: e2e-browser.sh started workspace despite lock"
    FAIL=$((FAIL + 1))
fi
# Held lock must still be intact
assert_eq "1" "$([ -f "$E2E_LOCK_DIR_OVERRIDE/e2e.lock" ] && echo 1 || echo 0)" "held lock preserved"
rm -f "$E2E_LOCK_DIR_OVERRIDE/e2e.lock"

# ── Summary ──────────────────────────────────────────────────────────────
echo ""
echo "Results: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
