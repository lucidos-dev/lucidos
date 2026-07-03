#!/bin/bash
# Tests for scripts/lib/host_load_guard.sh — the host-load backpressure guard that
# refuses to launch the Playwright browser swarm onto an already-saturated host.
# Run: ./scripts/lib/host_load_guard_test.sh   (no harness; direct, like webkit_reaper_test.sh)
#
# Hermetic + fast: load/core readings are injected via the HOST_LOAD_OVERRIDE /
# HOST_NCPU_OVERRIDE env hooks (never real sysctl), and the one case that needs the
# reading to CHANGE across polls (recovers mid-wait) redefines the reader function
# with a file-backed call counter — a plain shell var wouldn't survive the guard's
# command-substitution subshells. Poll/wait caps are tuned tiny so the whole suite
# runs in a couple of seconds. Exit codes are captured directly (cmd; rc=$?), never
# through a masking pipe.

set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

SANDBOX="$(mktemp -d)"
cleanup() { rm -rf "$SANDBOX"; }
trap cleanup EXIT

OUT="$SANDBOX/out"
mkdir -p "$OUT"

# shellcheck source=host_load_guard.sh
source "$SCRIPT_DIR/host_load_guard.sh"

PASS=0
FAIL=0
fail() { echo "  FAIL: $*"; FAIL=$((FAIL + 1)); }
pass() { echo "  ok:   $*"; PASS=$((PASS + 1)); }

assert_eq() {
    local expected="$1" actual="$2" msg="$3"
    if [ "$expected" = "$actual" ]; then
        pass "$msg"
    else
        fail "$msg (expected '$expected', got '$actual')"
    fi
}

# ── Test 1: ratio under threshold → returns 0 immediately (no wait) ─────────
test_under_threshold_immediate() {
    echo "test: ratio ≤ cap returns 0 immediately with a single sample"
    local rc start end elapsed
    start=$(date +%s)
    # 1.0 / 18 = 0.06x, well under a 1.5 cap. A long poll interval proves that a
    # wrongly-triggered wait would be obvious (it must NOT sleep 15s).
    HOST_LOAD_OVERRIDE=1.0 HOST_NCPU_OVERRIDE=18 \
        HOST_LOAD_MAX_RATIO=1.5 HOST_LOAD_POLL_SECS=15 \
        wait_for_host_load >"$OUT/low.out" 2>&1
    rc=$?
    end=$(date +%s)
    elapsed=$((end - start))
    assert_eq "0" "$rc" "under-threshold returns 0"
    if [ "$elapsed" -lt 3 ]; then
        pass "under-threshold did not wait (${elapsed}s)"
    else
        fail "under-threshold waited ${elapsed}s (should be immediate)"
    fi
}

# ── Test 2: ratio over threshold that never recovers → saturated exit 75 ────
test_never_recovers_saturated() {
    echo "test: sustained over-cap returns HOST_LOAD_SATURATED_EXIT (75) after the wait cap"
    local rc start end elapsed
    start=$(date +%s)
    # 40 / 18 = 2.22x, sustained. Tiny poll + wait caps keep the test fast.
    HOST_LOAD_OVERRIDE=40 HOST_NCPU_OVERRIDE=18 \
        HOST_LOAD_MAX_RATIO=1.5 HOST_LOAD_POLL_SECS=1 HOST_LOAD_MAX_WAIT_SECS=2 \
        wait_for_host_load >"$OUT/sat.out" 2>&1
    rc=$?
    end=$(date +%s)
    elapsed=$((end - start))
    assert_eq "75" "$rc" "sustained saturation returns 75"
    # Prove it respected the wait cap and did not hang (≈2s of polling + slack).
    if [ "$elapsed" -le 6 ]; then
        pass "respected the wait cap (${elapsed}s, no hang)"
    else
        fail "waited ${elapsed}s — exceeded the max-wait budget (possible hang)"
    fi
    if grep -q "still saturated" "$OUT/sat.out" && grep -q "refusing to launch" "$OUT/sat.out"; then
        pass "final message explains the refusal"
    else
        fail "final message missing 'still saturated … refusing to launch'"
        echo "  ---"; cat "$OUT/sat.out"; echo "  ---"
    fi
}

# ── Test 3: ratio over threshold that recovers mid-wait → returns 0 ─────────
# The reader must change across polls, which command-substitution subshells make
# impossible with a plain var — so use a file-backed call counter. Save + restore
# the real reader so later tests keep the env-hook behavior.
test_recovers_mid_wait() {
    echo "test: over-cap that recovers mid-wait returns 0"
    local rc real_reader counter
    counter="$SANDBOX/load-calls"
    echo 0 > "$counter"
    real_reader="$(declare -f _host_load_read_load1)"
    # Call 1: 40 (2.22x, over → enters the wait loop). Call 2+: 10 (0.56x, under).
    eval '_host_load_read_load1() {
        local n
        n=$(cat "'"$counter"'" 2>/dev/null || echo 0)
        n=$((n + 1))
        echo "$n" > "'"$counter"'"
        if [ "$n" -ge 2 ]; then echo "10.0"; else echo "40.0"; fi
    }'
    HOST_NCPU_OVERRIDE=18 HOST_LOAD_MAX_RATIO=1.5 \
        HOST_LOAD_POLL_SECS=1 HOST_LOAD_MAX_WAIT_SECS=30 \
        wait_for_host_load >"$OUT/recover.out" 2>&1
    rc=$?
    eval "$real_reader"   # restore the real reader
    assert_eq "0" "$rc" "recovery mid-wait returns 0"
    if grep -q "recovered" "$OUT/recover.out"; then
        pass "logged the recovery"
    else
        fail "recovery not logged"
        echo "  ---"; cat "$OUT/recover.out"; echo "  ---"
    fi
}

# ── Test 4: HOST_LOAD_GUARD_DISABLE=1 → no-op returns 0 ─────────────────────
test_disable_knob() {
    echo "test: HOST_LOAD_GUARD_DISABLE=1 is a no-op that returns 0"
    local rc start end elapsed
    start=$(date +%s)
    # Wildly over-cap (999/1) with a long poll: if the guard ran it would wait
    # forever. Disabled, it must return 0 instantly.
    HOST_LOAD_GUARD_DISABLE=1 HOST_LOAD_OVERRIDE=999 HOST_NCPU_OVERRIDE=1 \
        HOST_LOAD_MAX_RATIO=1.5 HOST_LOAD_POLL_SECS=15 \
        wait_for_host_load >"$OUT/dis.out" 2>&1
    rc=$?
    end=$(date +%s)
    elapsed=$((end - start))
    assert_eq "0" "$rc" "disabled guard returns 0"
    if [ "$elapsed" -lt 3 ]; then
        pass "disabled guard did not sample or wait (${elapsed}s)"
    else
        fail "disabled guard waited ${elapsed}s"
    fi
    if grep -q "disabled" "$OUT/dis.out"; then
        pass "logged that it was disabled"
    else
        fail "did not log the disabled state"
    fi
}

# ── Test 5: float compare exactness (awk, not bash arithmetic) ──────────────
test_float_compare_exactness() {
    echo "test: _host_load_over_ratio is float-exact at the boundary"
    local rc
    # 27 / 18 = 1.5 exactly — NOT over a 1.5 cap.
    _host_load_over_ratio 27 18 1.5; rc=$?
    assert_eq "1" "$rc" "27/18 = 1.5x is NOT over a 1.5 cap"
    # 27.1 / 18 = 1.5056x — IS over.
    _host_load_over_ratio 27.1 18 1.5; rc=$?
    assert_eq "0" "$rc" "27.1/18 = 1.506x IS over a 1.5 cap"
    # A tiny fractional overshoot above an integer core count.
    _host_load_over_ratio 18.5 18 1.0; rc=$?
    assert_eq "0" "$rc" "18.5/18 IS over a 1.0 cap"
    _host_load_over_ratio 18 18 1.0; rc=$?
    assert_eq "1" "$rc" "18/18 = 1.0x is NOT over a 1.0 cap"
}

# ── Test 6: empty / zero ncpu handled without a divide-by-zero crash ────────
test_zero_ncpu_fail_open() {
    echo "test: empty/zero ncpu fails open (no divide-by-zero, no false saturation)"
    local rc real_ncpu
    # Compare helper must not divide by zero — returns "not over".
    _host_load_over_ratio 40 0 1.5; rc=$?
    assert_eq "1" "$rc" "compare with ncpu=0 returns 'not over' (no crash)"
    # Non-numeric core count is also rejected.
    _host_load_over_ratio 40 abc 1.5; rc=$?
    assert_eq "1" "$rc" "compare with non-numeric ncpu returns 'not over'"
    # Ratio helper prints a safe sentinel rather than dividing by zero.
    assert_eq "?" "$(_host_load_ratio 40 0)" "ratio with ncpu=0 prints '?'"
    assert_eq "1.50" "$(_host_load_ratio 27 18)" "ratio with valid inputs prints 1.50"

    # Guard level: ncpu override of 0 → fail open (proceed), even under heavy load.
    HOST_LOAD_OVERRIDE=40 HOST_NCPU_OVERRIDE=0 \
        HOST_LOAD_MAX_RATIO=1.5 HOST_LOAD_POLL_SECS=1 HOST_LOAD_MAX_WAIT_SECS=5 \
        wait_for_host_load >"$OUT/ncpu0.out" 2>&1
    rc=$?
    assert_eq "0" "$rc" "guard with ncpu=0 proceeds (fails open)"
    if grep -q "could not determine CPU core count" "$OUT/ncpu0.out"; then
        pass "logged the unreadable core count"
    else
        fail "did not log the unreadable core count"
    fi

    # Guard level: an EMPTY core count (reader returns nothing) → also fail open.
    real_ncpu="$(declare -f _host_load_read_ncpu)"
    eval '_host_load_read_ncpu() { echo ""; }'
    HOST_LOAD_OVERRIDE=40 HOST_LOAD_MAX_RATIO=1.5 \
        HOST_LOAD_POLL_SECS=1 HOST_LOAD_MAX_WAIT_SECS=5 \
        wait_for_host_load >"$OUT/ncpu-empty.out" 2>&1
    rc=$?
    eval "$real_ncpu"   # restore the real reader
    assert_eq "0" "$rc" "guard with an empty core count proceeds (fails open)"
}

test_under_threshold_immediate
test_never_recovers_saturated
test_recovers_mid_wait
test_disable_knob
test_float_compare_exactness
test_zero_ncpu_fail_open

echo ""
echo "Passed: $PASS  Failed: $FAIL"
[ "$FAIL" -eq 0 ]
