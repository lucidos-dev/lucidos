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

# ── the mid-run sampler ─────────────────────────────────────────────────────
# The launch gate only knows about the instant it fired. These cover the second
# half: sampling THROUGH the run and classifying a failing run whose host went
# saturated afterwards, without retrying it or touching its exit code.
export HOST_LOAD_SAMPLES_FILE="$SANDBOX/samples"
export HOST_LOAD_SAMPLER_PIDFILE="$SANDBOX/sampler.pid"

# Write a synthetic sample series: N lines, INTERVAL seconds apart, each with the
# given load. Lets the report tests describe a 10-minute saturation in a
# millisecond instead of waiting for one.
write_samples() {
    local start="$1" interval="$2" count="$3" load="$4" i=0 ts
    while [ "$i" -lt "$count" ]; do
        ts=$((start + i * interval))
        printf '%s %s\n' "$ts" "$load" >> "$HOST_LOAD_SAMPLES_FILE"
        i=$((i + 1))
    done
}

test_sampler_records_and_stops() {
    echo "test: the sampler records samples during the run and stops cleanly"
    local pid lines
    rm -f "$HOST_LOAD_SAMPLES_FILE" "$HOST_LOAD_SAMPLER_PIDFILE"
    HOST_LOAD_OVERRIDE=40 HOST_LOAD_POLL_SECS=1 start_host_load_sampler >"$OUT/sampler.out" 2>&1
    pid="$HOST_LOAD_SAMPLER_PID"
    sleep 2
    stop_host_load_sampler

    if [ -n "$pid" ]; then pass "sampler started (pid=$pid)"; else fail "sampler did not start"; fi
    lines="$(wc -l < "$HOST_LOAD_SAMPLES_FILE" | tr -d ' ')"
    if [ "${lines:-0}" -ge 2 ]; then
        pass "recorded $lines samples in ~2s at a 1s interval"
    else
        fail "recorded only ${lines:-0} samples"
    fi
    if [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null; then
        fail "sampler still alive after stop (pid=$pid)"
        kill "$pid" 2>/dev/null || true
    else
        pass "sampler process gone after stop"
    fi
    if [ -f "$HOST_LOAD_SAMPLER_PIDFILE" ]; then fail "pidfile left behind"; else pass "pidfile removed"; fi
    # Samples survive the stop — the report drains them, not the teardown.
    if [ -f "$HOST_LOAD_SAMPLES_FILE" ]; then
        pass "samples kept for the report"
    else
        fail "samples deleted before the report could read them"
    fi
}

test_sampler_start_truncates_previous_run() {
    echo "test: starting the sampler discards a crashed predecessor's samples"
    rm -f "$HOST_LOAD_SAMPLER_PIDFILE"
    write_samples 1000 15 40 400      # a previous run's saturation
    HOST_LOAD_OVERRIDE=1.0 HOST_LOAD_POLL_SECS=1 start_host_load_sampler >/dev/null 2>&1
    stop_host_load_sampler
    if grep -q " 400$" "$HOST_LOAD_SAMPLES_FILE" 2>/dev/null; then
        fail "stale samples from a previous run survived"
    else
        pass "previous run's samples were truncated"
    fi
    rm -f "$HOST_LOAD_SAMPLES_FILE"
}

test_failed_run_with_sustained_saturation_banners() {
    echo "test: a FAILED run with sustained mid-run saturation gets the loud banner"
    local rc
    rm -f "$HOST_LOAD_SAMPLES_FILE"
    # 40 samples, 15s apart = 585s over cap: 100/18 = 5.56x, well over 1.5x.
    write_samples 1000 15 40 100
    HOST_NCPU_OVERRIDE=18 HOST_LOAD_MAX_RATIO=1.5 HOST_LOAD_SUSTAINED_MIN_SECS=120 \
        report_host_load_saturation 1 >"$OUT/banner.out" 2>&1
    rc=$?

    assert_eq "0" "$rc" "the reporter itself returns 0 (it reports, it does not decide)"
    if grep -q "HOST WAS SATURATED MID-RUN" "$OUT/banner.out"; then
        pass "printed the saturation banner"
    else
        fail "no banner on a failed, saturated run"; cat "$OUT/banner.out"
    fi
    # 5.56x is load/cores, NOT a multiple of the 1.5x cap — the banner must not
    # conflate the two ("5.56x the 1.50x cap" would claim 8.34x).
    if grep -q "peak load 100.00 on 18 cores = 5.56x, against a 1.50x cap" "$OUT/banner.out" \
        && grep -q "sustained above that cap for 10 min" "$OUT/banner.out"; then
        pass "banner quantifies the peak and the sustained duration, without conflating ratio and cap"
    else
        fail "banner missing peak/duration, or misstates the ratio"; cat "$OUT/banner.out"
    fi
    if grep -q "RE-RUN ON AN IDLE HOST" "$OUT/banner.out" \
        && grep -q "exit code is unchanged (1)" "$OUT/banner.out" \
        && grep -q "has NOT been retried" "$OUT/banner.out"; then
        pass "banner says re-run, and that nothing was retried or suppressed"
    else
        fail "banner missing the re-run / no-suppression wording"; cat "$OUT/banner.out"
    fi
    if [ -f "$HOST_LOAD_SAMPLES_FILE" ]; then fail "samples not drained"; else pass "samples drained"; fi
}

test_failed_run_with_brief_spike_does_not_banner() {
    echo "test: a FAILED run with only a brief load spike is NOT blamed on the host"
    rm -f "$HOST_LOAD_SAMPLES_FILE"
    write_samples 1000 15 20 10       # 0.56x — quiet
    write_samples 1300 15 3  100      # 30s over cap — a spike, not saturation
    write_samples 1400 15 20 10
    HOST_NCPU_OVERRIDE=18 HOST_LOAD_MAX_RATIO=1.5 HOST_LOAD_SUSTAINED_MIN_SECS=120 \
        report_host_load_saturation 1 >"$OUT/spike.out" 2>&1

    if grep -q "HOST WAS SATURATED MID-RUN" "$OUT/spike.out"; then
        fail "banner fired on a 30s spike (would cry wolf on real failures)"
        cat "$OUT/spike.out"
    else
        pass "no banner for a spike shorter than the sustained window"
    fi
    if grep -q "mid-run host load: peak 100.00" "$OUT/spike.out"; then
        pass "still printed the one-line load summary as evidence"
    else
        fail "no load summary"; cat "$OUT/spike.out"
    fi
}

test_passing_run_never_banners() {
    echo "test: a PASSING run never gets the banner, however saturated the host was"
    rm -f "$HOST_LOAD_SAMPLES_FILE"
    write_samples 1000 15 40 100
    HOST_NCPU_OVERRIDE=18 HOST_LOAD_MAX_RATIO=1.5 HOST_LOAD_SUSTAINED_MIN_SECS=120 \
        report_host_load_saturation 0 >"$OUT/green.out" 2>&1

    if grep -q "HOST WAS SATURATED MID-RUN" "$OUT/green.out"; then
        fail "banner fired on a green run — it explains failures, it doesn't warn"
    else
        pass "no banner on a green run"
    fi
    if grep -q "mid-run host load: peak" "$OUT/green.out"; then
        pass "load summary still recorded for the log"
    else
        fail "no load summary on a green run"
    fi
}

test_banner_uses_the_launch_gate_threshold() {
    echo "test: the banner keys on HOST_LOAD_MAX_RATIO — the same cap as the launch gate"
    rm -f "$HOST_LOAD_SAMPLES_FILE"
    write_samples 1000 15 40 100      # 5.56x on 18 cores
    # Raise the ONE cap above the observed ratio: the same samples must now be
    # judged fine. If the sampler had its own private threshold, this would fail.
    HOST_NCPU_OVERRIDE=18 HOST_LOAD_MAX_RATIO=10 HOST_LOAD_SUSTAINED_MIN_SECS=120 \
        report_host_load_saturation 1 >"$OUT/cap.out" 2>&1

    if grep -q "HOST WAS SATURATED MID-RUN" "$OUT/cap.out"; then
        fail "banner ignored the raised HOST_LOAD_MAX_RATIO cap"; cat "$OUT/cap.out"
    else
        pass "raising HOST_LOAD_MAX_RATIO suppressed the banner (one shared cap)"
    fi
    if grep -q "0/40 samples over the 10.00x cap" "$OUT/cap.out"; then
        pass "summary reports against the same cap"
    else
        fail "summary did not use the configured cap"; cat "$OUT/cap.out"
    fi
}

test_report_fails_open_without_samples() {
    echo "test: the report fails open when it cannot measure"
    local rc
    rm -f "$HOST_LOAD_SAMPLES_FILE"
    # No samples file at all (sampler never started / disabled).
    report_host_load_saturation 1 >"$OUT/nofile.out" 2>&1
    rc=$?
    assert_eq "0" "$rc" "no samples file → returns 0, prints nothing"
    if [ -s "$OUT/nofile.out" ]; then fail "printed output with no samples"; else pass "silent with no samples"; fi

    # An empty / garbage sample set is also not evidence of anything.
    printf 'garbage\n\n' > "$HOST_LOAD_SAMPLES_FILE"
    HOST_NCPU_OVERRIDE=18 report_host_load_saturation 1 >"$OUT/garbage.out" 2>&1
    rc=$?
    assert_eq "0" "$rc" "unusable samples → returns 0"
    if grep -q "HOST WAS SATURATED" "$OUT/garbage.out"; then
        fail "bannered on unparseable samples"
    else
        pass "no banner on unparseable samples"
    fi

    # Unreadable core count → say so, don't guess.
    write_samples 1000 15 40 100
    HOST_NCPU_OVERRIDE=0 report_host_load_saturation 1 >"$OUT/noncpu.out" 2>&1
    rc=$?
    assert_eq "0" "$rc" "unreadable core count → returns 0"
    if grep -q "core count unreadable" "$OUT/noncpu.out"; then
        pass "logged that it could not classify"
    else
        fail "silent about an unreadable core count"; cat "$OUT/noncpu.out"
    fi
}

test_sampler_disabled_with_the_guard() {
    echo "test: HOST_LOAD_GUARD_DISABLE also disables the mid-run sampler"
    rm -f "$HOST_LOAD_SAMPLES_FILE" "$HOST_LOAD_SAMPLER_PIDFILE"
    HOST_LOAD_SAMPLER_PID=""
    # Assert on the PIDFILE, not on $HOST_LOAD_SAMPLER_PID. A `VAR=x func` prefix
    # assignment is restored when the function returns, so reading the variable
    # afterwards says "empty" even if a sampler really spawned — a check that can
    # only ever pass. The pidfile is written by the real start path, so it is the
    # honest witness.
    HOST_LOAD_GUARD_DISABLE=1 HOST_LOAD_OVERRIDE=999 \
        start_host_load_sampler >"$OUT/sdis.out" 2>&1
    if [ -f "$HOST_LOAD_SAMPLER_PIDFILE" ]; then
        fail "sampler started while the guard was disabled (pidfile written)"
        stop_host_load_sampler
    else
        pass "sampler not started while the guard was disabled (no pidfile)"
    fi
    if grep -q "disabled" "$OUT/sdis.out"; then
        pass "logged the disabled sampler"
    else
        fail "did not log the disabled sampler"
    fi
}

test_disabled_run_cannot_inherit_a_predecessors_samples() {
    echo "test: a disabled run never reports a crashed predecessor's saturation"
    # stop_host_load_sampler deliberately keeps the samples for the report, so an
    # interrupted run leaves a saturated file on disk. A later run with the guard
    # disabled must not pick it up and banner about load it never experienced.
    rm -f "$HOST_LOAD_SAMPLER_PIDFILE"
    write_samples 1000 15 40 100

    HOST_LOAD_GUARD_DISABLE=1 start_host_load_sampler >/dev/null 2>&1
    if [ -f "$HOST_LOAD_SAMPLES_FILE" ]; then
        fail "predecessor's samples survived the disabled start"
    else
        pass "disabled start discarded the predecessor's samples"
    fi

    # Belt and braces: even if a file reappears, the report refuses to classify
    # while the guard is disabled.
    write_samples 1000 15 40 100
    HOST_NCPU_OVERRIDE=18 HOST_LOAD_GUARD_DISABLE=1 \
        report_host_load_saturation 1 >"$OUT/disrep.out" 2>&1
    if [ -s "$OUT/disrep.out" ]; then
        fail "the report spoke while the guard was disabled"; cat "$OUT/disrep.out"
    else
        pass "the report stayed silent while the guard was disabled"
    fi
    if [ -f "$HOST_LOAD_SAMPLES_FILE" ]; then
        fail "stale samples left for the next run"
    else
        pass "stale samples discarded"
    fi
}

test_start_reaps_an_orphaned_predecessor() {
    echo "test: starting the sampler reaps an orphan left by a SIGKILLed run"
    # The loop is disowned, so a killed e2e-browser.sh leaves it appending
    # forever; two samplers interleaving into one file would describe a run that
    # never happened.
    rm -f "$HOST_LOAD_SAMPLES_FILE"
    sleep 120 &
    local orphan=$!
    echo "$orphan" > "$HOST_LOAD_SAMPLER_PIDFILE"
    HOST_LOAD_SAMPLER_PID="" HOST_LOAD_OVERRIDE=1.0 HOST_LOAD_POLL_SECS=1 \
        start_host_load_sampler >/dev/null 2>&1
    stop_host_load_sampler

    if kill -0 "$orphan" 2>/dev/null; then
        fail "orphan from the pidfile survived the new start (pid=$orphan)"
        kill "$orphan" 2>/dev/null || true
    else
        pass "orphan recorded in the pidfile was reaped before starting"
    fi
    rm -f "$HOST_LOAD_SAMPLES_FILE"
}

test_under_threshold_immediate
test_never_recovers_saturated
test_recovers_mid_wait
test_disable_knob
test_float_compare_exactness
test_zero_ncpu_fail_open
test_sampler_records_and_stops
test_sampler_start_truncates_previous_run
test_failed_run_with_sustained_saturation_banners
test_failed_run_with_brief_spike_does_not_banner
test_passing_run_never_banners
test_banner_uses_the_launch_gate_threshold
test_report_fails_open_without_samples
test_sampler_disabled_with_the_guard
test_disabled_run_cannot_inherit_a_predecessors_samples
test_start_reaps_an_orphaned_predecessor

echo ""
echo "Passed: $PASS  Failed: $FAIL"
[ "$FAIL" -eq 0 ]
