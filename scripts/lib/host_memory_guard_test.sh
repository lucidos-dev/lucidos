#!/bin/bash
# Tests for scripts/lib/host_memory_guard.sh, the host-memory stop condition that
# ends a browser e2e run at a chunk boundary when the host is out of memory to lend.
# Run: ./scripts/lib/host_memory_guard_test.sh   (no harness; direct, like host_load_guard_test.sh)
#
# Hermetic: every reading is injected through the HOST_COMPRESSOR_GB_OVERRIDE /
# HOST_AVAIL_GB_OVERRIDE / HOST_SWAP_USED_GB_OVERRIDE / HOST_PHYSMEM_GB_OVERRIDE /
# HOST_PRESSURE_LEVEL_OVERRIDE seams, never a real vm_stat or sysctl. Every
# boundary and start call injects
# available memory too, so the scarcity floor is exercised on purpose and never by
# the CI host's mood. The cases that exercise PARSING instead of policy shadow
# `vm_stat` and `sysctl` as functions, so the awk is fed known text. Exit codes are
# captured directly (cmd; rc=$?), never through a masking pipe.
#
# TWO TESTS RUN A SCRIPT THAT IS NOT IN THIS REPO. The pre-flight gate is a
# knowhow script in the ops workspace, and the running guard must agree with it.
# Those two shadow `vm_stat`, `sysctl` and `ps` on PATH instead of as functions,
# because the gate runs as its own process. They read the real host no more than
# the rest do, and they SKIP loudly when the gate is not on this machine.

set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

SANDBOX="$(mktemp -d)"
cleanup() { rm -rf "$SANDBOX"; }
trap cleanup EXIT

OUT="$SANDBOX/out"
mkdir -p "$OUT"

# shellcheck source=host_memory_guard.sh
source "$SCRIPT_DIR/host_memory_guard.sh"

# Keep the suite hermetic. The boundary check now calls _host_mem_attr_report,
# which would otherwise run real ps / footprint / lsof against THIS host. An empty
# candidate list is enough to keep it hermetic: with no candidates it never calls
# footprint or lsof, and it degrades to its own note, which every existing
# assertion is blind to (it names no STOP, cap, floor or swap wording). The
# attribution tests below run LAST and install synthetic feeds, so every test
# before them stays hermetic.
# shellcheck disable=SC2329 # a seam: invoked by the sourced guard, not from this file
_host_mem_attr_proc_list() { return 0; }

# Point the attribution's engine-pidfile read and cwd resolve at the sandbox, not
# the real ~/workspaces/e2e-test, so a test host with a live e2e run cannot leak
# into these results. The attribution tests below override this per test.
export E2E_WORKSPACE="$SANDBOX/e2e-test"

# Pin the browser-cache discriminator to its default token, so the WebKit
# classification stays hermetic. The guard reads PLAYWRIGHT_BROWSERS_PATH to
# decide whether a browser path belongs to the run. A value inherited from the
# test host would not match the synthetic fixture paths, flipping every synthetic
# browser to browser-host. The fixtures embed the default "ms-playwright" token,
# so unset it. Same posture as webkit_reaper_test.sh and e2e_lock_test.sh.
unset PLAYWRIGHT_BROWSERS_PATH

# Pressure is now a stop condition, so it must be pinned for the WHOLE file or
# every test inherits the CI host's mood: a host sitting at critical would stop
# the floor and swap cases on the wrong reason. Normal is the neutral value; the
# pressure tests below set their own.
export HOST_PRESSURE_LEVEL_OVERRIDE=1

# The boundary check folds the peak sampler's window over its own reading, so the
# samples file must point at the sandbox. Left at its default it would read a real
# run's file on a host that has one, and a stale sample could stop a test.
export HOST_MEMORY_SAMPLES_FILE="$SANDBOX/host-memory-samples"
export HOST_MEMORY_SAMPLER_PIDFILE="$SANDBOX/host-memory-sampler.pid"
# The in-chunk stop's two files, for the same reason: a real run's runner pidfile
# must never be read here, let alone signalled.
export HOST_MEMORY_TRIP_FILE="$SANDBOX/host-memory-trip"
export HOST_MEMORY_RUNNER_PIDFILE="$SANDBOX/host-memory-runner.pid"

# A critical reading is now re-sampled before it is believed, up to 8 times 5 s
# apart. Shadow the confirm's wait seam for the WHOLE file and record what it was
# asked to wait, so the suite stays instant and the spacing is still assertable.
# The seam has exactly one caller, so nothing else changes behaviour.
CONFIRM_WAITS="$SANDBOX/confirm-waits"
: > "$CONFIRM_WAITS"
# shellcheck disable=SC2329 # a seam: invoked by the sourced guard, not from this file
_host_mem_confirm_wait() { printf '%s\n' "$1" >> "$CONFIRM_WAITS"; }

# How many times the confirm waited, and at what interval. Both are read after a
# boundary call, and a stop that waited nothing is as much a finding as one that
# waited the whole window.
confirm_wait_count() { wc -l < "$CONFIRM_WAITS" | tr -d ' '; }
confirm_wait_intervals() { sort -u "$CONFIRM_WAITS" | tr '\n' ' ' | sed 's/ $//'; }

# Feed a SEQUENCE of pressure levels, one per read, with the last one repeating.
# The boundary's own read takes the first and the confirm loop takes the rest, so
# a test can replay a host whose level oscillates. `x` means the oid could not be
# read. Call it INSIDE a subshell: it shadows the reader, and a shadow that
# leaked would silently blind every test after it.
#
# The index lives in a FILE because every read happens inside a command
# substitution, where an incremented shell variable would not survive.
PRESSURE_SEQ_FILE="$SANDBOX/pressure-seq"
PRESSURE_SEQ_IDX="$SANDBOX/pressure-seq-idx"
use_pressure_sequence() {
    printf '%s\n' "$*" > "$PRESSURE_SEQ_FILE"
    printf '0\n' > "$PRESSURE_SEQ_IDX"
    # shellcheck disable=SC2329 # a seam: invoked by the sourced guard, not from this file
    _host_mem_read_pressure_level() {
        local i seq
        i="$(cat "$PRESSURE_SEQ_IDX" 2>/dev/null || echo 0)"
        printf '%s\n' "$((i + 1))" > "$PRESSURE_SEQ_IDX"
        seq="$(cat "$PRESSURE_SEQ_FILE")"
        awk -v s="$seq" -v i="$i" 'BEGIN {
            n = split(s, a, " ")
            v = (i + 1 <= n) ? a[i + 1] : a[n]
            if (v != "x") printf "%s", v
        }'
    }
}

PASS=0
FAIL=0
# A skip is never a pass. Only the two pre-flight gate tests can skip, and only
# when that script is not on this machine, so the count keeps a partial run from
# reading as full coverage.
SKIPPED=0
fail() { echo "  FAIL: $*"; FAIL=$((FAIL + 1)); }
pass() { echo "  ok:   $*"; PASS=$((PASS + 1)); }
skip() { echo "  SKIP: $*"; SKIPPED=$((SKIPPED + 1)); }

assert_eq() {
    local expected="$1" actual="$2" msg="$3"
    if [ "$expected" = "$actual" ]; then
        pass "$msg"
    else
        fail "$msg (expected '$expected', got '$actual')"
    fi
}

assert_says() {
    local file="$1" needle="$2" msg="$3"
    if grep -qF -- "$needle" "$file"; then
        pass "$msg"
    else
        fail "$msg (no '$needle' in output)"
        sed 's/^/      | /' "$file"
    fi
}

assert_silent_about() {
    local file="$1" needle="$2" msg="$3"
    if grep -qF -- "$needle" "$file"; then
        fail "$msg (found '$needle')"
        sed 's/^/      | /' "$file"
    else
        pass "$msg"
    fi
}

# Reset the guard's recorded state between tests. The real caller sources this
# once per run, so a leaked MEMORY_STOPPED would make a later test report a stop
# that never happened.
reset_state() {
    MEMORY_STOPPED=""
    MEMORY_STOP_DETAIL=""
    HOST_MEMORY_BASELINE_GB=""
    HOST_MEMORY_STOP_COMPRESSOR_GB=""
    HOST_MEMORY_SAMPLER_PID=""
    HOST_MEMORY_RUNNER_PID=""
    # A window left behind by the peak-sampler tests would be folded into the next
    # test's boundary reading, which is exactly the cross-test leak reset_state
    # exists to prevent.
    rm -f "$HOST_MEMORY_SAMPLES_FILE" "$HOST_MEMORY_SAMPLER_PIDFILE" \
        "$HOST_MEMORY_TRIP_FILE" "$HOST_MEMORY_RUNNER_PIDFILE" 2>/dev/null || true
    # Same reason: a previous test's confirm waits would be counted as this one's.
    : > "$CONFIRM_WAITS"
}

# ── Test 1: the regression. A busy host that used to be stopped now runs on ──
test_the_old_ceiling_no_longer_stops_a_healthy_host() {
    echo "test: 12.25 GB compressor with no swap and ample free on a 48 GB host runs on"
    # A reading that a low fixed ceiling used to stop, with swap 0 and free memory
    # far above the floor, so the host was fine.
    local rc
    reset_state
    HOST_COMPRESSOR_GB_OVERRIDE=12.25 HOST_AVAIL_GB_OVERRIDE=30.00 \
        HOST_SWAP_USED_GB_OVERRIDE=0.00 HOST_PHYSMEM_GB_OVERRIDE=48 \
        check_host_memory_at_boundary "nav chunk 20/32" >"$OUT/reg.out" 2>&1
    rc=$?
    assert_eq "0" "$rc" "the run continues past a low fixed ceiling"
    assert_says "$OUT/reg.out" "pressure normal, compressor 12.25 GB, available 30.00 GB, swap 0.00 GB" "the boundary line states all four numbers"
    assert_silent_about "$OUT/reg.out" "STOP" "no stop was announced"
}

# ── the reading that stopped tonight's nightly now runs on ──────────────────
test_tonights_reading_runs_on() {
    echo "test: 16.69 GB compressor, 27 GB free, swap 0 on a 48 GB host runs on"
    # The exact boundary that ended the unfiltered mobile-webkit project at nav
    # chunk 27 of 34 with exit 71: swap 0.00, about 57% (27 GB) free on 48 GB. The
    # flat 16 GB cap fired while the host was healthy. It no longer exists.
    local rc
    reset_state
    HOST_COMPRESSOR_GB_OVERRIDE=16.69 HOST_AVAIL_GB_OVERRIDE=27 \
        HOST_SWAP_USED_GB_OVERRIDE=0.00 HOST_PHYSMEM_GB_OVERRIDE=48 \
        check_host_memory_at_boundary "nav chunk 27/34" >"$OUT/tonight.out" 2>&1
    rc=$?
    assert_eq "0" "$rc" "the boundary that ended tonight's run no longer stops"
    assert_silent_about "$OUT/tonight.out" "STOP" "no stop was announced"
}

# ── Test 2: swap in use is the real distress stop ───────────────────────────
test_swap_over_the_limit_stops() {
    echo "test: swap over the limit stops the run, whatever the compressor says"
    local rc
    reset_state
    # A modest compressor and ample free memory, so only swap can be what stopped it.
    HOST_COMPRESSOR_GB_OVERRIDE=6.00 HOST_AVAIL_GB_OVERRIDE=30 \
        HOST_SWAP_USED_GB_OVERRIDE=3.50 HOST_PHYSMEM_GB_OVERRIDE=48 \
        check_host_memory_at_boundary "nav chunk 4/32" >"$OUT/swap.out" 2>&1
    rc=$?
    assert_eq "1" "$rc" "swap over the limit returns non-zero"
    assert_says "$OUT/swap.out" "STOP: 3.50 GB of swap is in use" "the stop names swap as the cause"
    assert_says "$OUT/swap.out" "over the 1 GB limit" "the stop states the limit it broke"
    case "$MEMORY_STOP_DETAIL" in
        *"nav chunk 4/32"*"3.50 GB of swap"*)
            pass "the recorded detail names the boundary and the reading" ;;
        *) fail "detail did not record the boundary and reading: '$MEMORY_STOP_DETAIL'" ;;
    esac
}

test_swap_at_the_limit_does_not_stop() {
    echo "test: swap exactly at the limit is allowed through"
    # The comparison is strictly greater, so the limit is the last passing value.
    local rc
    reset_state
    HOST_COMPRESSOR_GB_OVERRIDE=6.00 HOST_AVAIL_GB_OVERRIDE=30 \
        HOST_SWAP_USED_GB_OVERRIDE=1.00 HOST_PHYSMEM_GB_OVERRIDE=48 LUCIDOS_E2E_SWAP_MAX_GB=1 \
        check_host_memory_at_boundary "nav chunk 5/32" >"$OUT/swapeq.out" 2>&1
    rc=$?
    assert_eq "0" "$rc" "swap equal to the limit returns 0"
}

test_swap_wins_when_all_are_over() {
    echo "test: with swap, scarcity and compressor all over, the message names swap"
    local rc
    reset_state
    HOST_COMPRESSOR_GB_OVERRIDE=40.00 HOST_AVAIL_GB_OVERRIDE=1.00 \
        HOST_SWAP_USED_GB_OVERRIDE=9.00 HOST_PHYSMEM_GB_OVERRIDE=48 \
        check_host_memory_at_boundary "cc chunk 1/4" >"$OUT/both.out" 2>&1
    rc=$?
    assert_eq "1" "$rc" "all over returns non-zero"
    assert_says "$OUT/both.out" "STOP: 9.00 GB of swap is in use" "swap is reported as the cause"
    assert_silent_about "$OUT/both.out" "backstop" "the backstop message is not also printed"
    assert_silent_about "$OUT/both.out" "CORROBORATED SCARCITY" "the scarcity message is not also printed"
}

# ── Test 3: the corroborated free-headroom floor ────────────────────────────
# A low available reading is necessary and never sufficient. The floor stops only
# when it is sustained AND a signal that measures the host rather than the
# compressed-page pool agrees. The two corroborators get one test each.
test_warn_pressure_corroborates_the_floor_and_stops() {
    echo "test: available under the floor with the kernel at warn stops the run"
    local rc
    reset_state
    # Compressor and swap both fine, so only the corroborated floor can be what
    # stopped it, and warn pressure is the only thing corroborating.
    HOST_PRESSURE_LEVEL_OVERRIDE=2 HOST_COMPRESSOR_GB_OVERRIDE=8.00 HOST_AVAIL_GB_OVERRIDE=3.00 \
        HOST_SWAP_USED_GB_OVERRIDE=0.00 HOST_PHYSMEM_GB_OVERRIDE=48 \
        check_host_memory_at_boundary "nav chunk 15/32" >"$OUT/scarce.out" 2>&1
    rc=$?
    assert_eq "1" "$rc" "available under the floor with warn pressure returns non-zero"
    assert_says "$OUT/scarce.out" "STOP: available memory 3.00 GB is under the 9.60 GB floor" "the stop names the floor it broke"
    assert_says "$OUT/scarce.out" "corroborated by the kernel at warn pressure" "the stop names its corroborator"
    assert_says "$OUT/scarce.out" "CORROBORATED SCARCITY" "the stop names itself as corroborated scarcity"
    assert_silent_about "$OUT/scarce.out" "REAL SCARCITY" "the retired wording is gone"
    case "$MEMORY_STOP_DETAIL" in
        *"nav chunk 15/32"*"available memory was 3.00 GB"*"corroborated by the kernel at warn"*)
            pass "the detail records the boundary, the reading and the corroborator" ;;
        *) fail "detail did not record the boundary, reading and corroborator: '$MEMORY_STOP_DETAIL'" ;;
    esac
}

test_swap_in_use_corroborates_the_floor_and_stops() {
    echo "test: available under the floor with any swap in use stops the run"
    local rc
    reset_state
    # 0.25 GB of swap is well under the 1 GB ceiling that stops on its own, so the
    # swap stop cannot be what fired. It corroborates from the first byte.
    HOST_PRESSURE_LEVEL_OVERRIDE=1 HOST_COMPRESSOR_GB_OVERRIDE=8.00 HOST_AVAIL_GB_OVERRIDE=3.00 \
        HOST_SWAP_USED_GB_OVERRIDE=0.25 HOST_PHYSMEM_GB_OVERRIDE=48 \
        check_host_memory_at_boundary "nav chunk 16/32" >"$OUT/swapcorr.out" 2>&1
    rc=$?
    assert_eq "1" "$rc" "available under the floor with swap in use returns non-zero"
    assert_says "$OUT/swapcorr.out" "corroborated by 0.25 GB of swap in use" "the stop names swap as its corroborator"
    assert_says "$OUT/swapcorr.out" "CORROBORATED SCARCITY" "the stop names itself as corroborated scarcity"
    assert_silent_about "$OUT/swapcorr.out" "MEASURED DISTRESS" "the swap stop itself did not fire"
}

# The regression this whole change removes. Deep scarcity on the available
# instrument alone, with the kernel calm and no swap, is exactly what an idle
# host reads. It must be recorded and never acted on, however far under it is.
test_an_uncorroborated_reading_never_stops_however_deep() {
    echo "test: 3.00 GB available with pressure normal and swap 0 runs on"
    local rc
    reset_state
    HOST_PRESSURE_LEVEL_OVERRIDE=1 HOST_COMPRESSOR_GB_OVERRIDE=8.00 HOST_AVAIL_GB_OVERRIDE=3.00 \
        HOST_SWAP_USED_GB_OVERRIDE=0.00 HOST_PHYSMEM_GB_OVERRIDE=48 \
        check_host_memory_at_boundary "nav chunk 17/32" >"$OUT/uncorr.out" 2>&1
    rc=$?
    assert_eq "0" "$rc" "an uncorroborated sub-floor reading does not stop the run"
    assert_silent_about "$OUT/uncorr.out" "STOP:" "no stop was announced"
    assert_says "$OUT/uncorr.out" "note: available dipped to 3.00 GB, under the 9.60 GB floor" "the reading is still recorded"
    assert_says "$OUT/uncorr.out" "Recorded, not a stop: it is uncorroborated." "the note says which half declined"
    assert_says "$OUT/uncorr.out" "Pressure normal at the boundary" "the note names the signal that declined"
    assert_says "$OUT/uncorr.out" "swap 0.00 GB" "the note names the other signal that declined"
    assert_silent_about "$OUT/uncorr.out" "it is not sustained" "the sustain half is not blamed, because it held"
}

# The fail-open contract, applied to the corroboration. An unreadable pressure
# oid corroborates nothing, and a guard that cannot measure the host must never
# be able to end a run.
test_unreadable_pressure_corroborates_nothing() {
    echo "test: available under the floor with pressure unreadable runs on"
    local rc
    reset_state
    # A subshell, so dropping the file-wide pressure pin cannot leak forward. Swap
    # is injected, so the stubbed sysctl reaches nothing but the pressure oid.
    (
        unset HOST_PRESSURE_LEVEL_OVERRIDE
        # shellcheck disable=SC2329 # a seam: invoked by the sourced guard, not from this file
        sysctl() { return 1; }
        HOST_COMPRESSOR_GB_OVERRIDE=8.00 HOST_AVAIL_GB_OVERRIDE=3.00 \
            HOST_SWAP_USED_GB_OVERRIDE=0.00 HOST_PHYSMEM_GB_OVERRIDE=48 \
            check_host_memory_at_boundary "nav chunk 18/32"
    ) >"$OUT/nocorr.out" 2>&1
    rc=$?
    assert_eq "0" "$rc" "an unreadable corroborator does not stop the run"
    assert_silent_about "$OUT/nocorr.out" "STOP:" "no stop was announced"
    assert_says "$OUT/nocorr.out" "Pressure unreadable at the boundary" "the note names the corroboration it could not read"
}

test_available_at_the_floor_does_not_stop() {
    echo "test: available exactly at the floor is allowed through"
    # Strictly greater, so available equal to the floor is the last passing value.
    local rc
    reset_state
    HOST_COMPRESSOR_GB_OVERRIDE=8.00 HOST_AVAIL_GB_OVERRIDE=9.60 \
        HOST_SWAP_USED_GB_OVERRIDE=0.00 HOST_PHYSMEM_GB_OVERRIDE=48 \
        check_host_memory_at_boundary "nav chunk 16/32" >"$OUT/flooreq.out" 2>&1
    rc=$?
    assert_eq "0" "$rc" "available equal to the floor returns 0"
}

test_the_floor_scales_with_ram() {
    echo "test: the same available reading stops a 48 GB host and not a 16 GB one"
    # 9 GB available is under the 9.6 GB floor of a 48 GB host but over the 8 GB
    # floor of a 16 GB one. So the same reading is danger on one machine and fine on
    # the other, which a single fixed number could never express. Compressor 5.00 GB
    # stays under both hosts' caps (16.8 and 5.6), so only the floor is in play.
    # Warn pressure corroborates on both, so the RAM share is the only difference.
    local rc
    reset_state
    HOST_PRESSURE_LEVEL_OVERRIDE=2 HOST_COMPRESSOR_GB_OVERRIDE=5.00 HOST_AVAIL_GB_OVERRIDE=9.00 \
        HOST_SWAP_USED_GB_OVERRIDE=0.00 HOST_PHYSMEM_GB_OVERRIDE=48 \
        check_host_memory_at_boundary "nav chunk 9/32" >"$OUT/floorbig.out" 2>&1
    rc=$?
    assert_eq "1" "$rc" "9 GB available on a 48 GB host stops"
    assert_says "$OUT/floorbig.out" "under the 9.60 GB floor" "the floor is a share of 48 GB RAM"

    reset_state
    HOST_PRESSURE_LEVEL_OVERRIDE=2 HOST_COMPRESSOR_GB_OVERRIDE=5.00 HOST_AVAIL_GB_OVERRIDE=9.00 \
        HOST_SWAP_USED_GB_OVERRIDE=0.00 HOST_PHYSMEM_GB_OVERRIDE=16 \
        check_host_memory_at_boundary "nav chunk 9/32" >"$OUT/floorsmall.out" 2>&1
    rc=$?
    assert_eq "0" "$rc" "9 GB available on a 16 GB host runs on (floor is 8 GB)"
}

test_the_floor_value_resolves_per_host() {
    echo "test: the floor is max(8 GB, 20% of RAM) and echoes per host"
    local got
    reset_state
    got="$(HOST_PHYSMEM_GB_OVERRIDE=48 _host_mem_available_floor_gb)"
    assert_eq "9.60" "$got" "48 GB resolves to 9.60 (the share beats the 8 GB minimum)"

    got="$(HOST_PHYSMEM_GB_OVERRIDE=16 _host_mem_available_floor_gb)"
    assert_eq "8.00" "$got" "16 GB resolves to 8.00 (the minimum beats the 3.2 GB share)"
}

test_the_floor_uses_the_minimum_when_ram_is_unreadable() {
    echo "test: with RAM unreadable the floor is the absolute minimum, not gone"
    local got rc
    reset_state
    # Shadow sysctl so hw.memsize is unreadable. A floor in bytes needs no RAM
    # total, so the minimum still applies and the scarcity guard does not lapse the
    # way the compressor backstop does. Swap and available are injected, so shadowing
    # sysctl only removes the physical-memory reading.
    # shellcheck disable=SC2329 # a seam: invoked by the sourced guard, not from this file
    sysctl() { return 1; }
    got="$(_host_mem_available_floor_gb)"
    assert_eq "8" "$got" "no RAM reading falls back to the 8 GB minimum"

    HOST_PRESSURE_LEVEL_OVERRIDE=2 HOST_COMPRESSOR_GB_OVERRIDE=9.00 HOST_AVAIL_GB_OVERRIDE=3.00 \
        HOST_SWAP_USED_GB_OVERRIDE=0.00 \
        check_host_memory_at_boundary "nav chunk 10/32" >"$OUT/floornoram.out" 2>&1
    rc=$?
    unset -f sysctl
    assert_eq "1" "$rc" "3 GB available stops even when RAM is unreadable"
    assert_says "$OUT/floornoram.out" "under the 8 GB floor" "the minimum floor still applies"
}

test_scarcity_wins_over_the_compressor_backstop() {
    echo "test: with available low and compressor high, the message names scarcity"
    local rc
    reset_state
    HOST_PRESSURE_LEVEL_OVERRIDE=2 HOST_COMPRESSOR_GB_OVERRIDE=30.00 HOST_AVAIL_GB_OVERRIDE=2.00 \
        HOST_SWAP_USED_GB_OVERRIDE=0.00 HOST_PHYSMEM_GB_OVERRIDE=48 \
        check_host_memory_at_boundary "nav chunk 28/34" >"$OUT/scarcewins.out" 2>&1
    rc=$?
    assert_eq "1" "$rc" "both over returns non-zero"
    assert_says "$OUT/scarcewins.out" "CORROBORATED SCARCITY" "scarcity is reported as the cause"
    assert_silent_about "$OUT/scarcewins.out" "RUNAWAY BACKSTOP" "the runaway message is not also printed"
    assert_silent_about "$OUT/scarcewins.out" "SURVIVABILITY" "the cap message is not also printed"
}

# The 2026-07-26 wedge reading. That night the compressor hit 17.41 GB with swap
# still clear, and the kernel went critical. This replays the same compressor
# reading with the kernel one level lower, at warn, so the critical stop cannot
# fire and the corroborated floor is what has to catch it. It asserts the floor
# is ahead of both compressor ceilings in the order.
test_the_2026_07_26_wedge_stops_on_the_corroborated_floor() {
    echo "test: compressor 17.41 GB, swap 0, available scarce under warn stops on the floor"
    local rc
    reset_state
    HOST_PRESSURE_LEVEL_OVERRIDE=2 HOST_COMPRESSOR_GB_OVERRIDE=17.41 HOST_AVAIL_GB_OVERRIDE=5.00 \
        HOST_SWAP_USED_GB_OVERRIDE=0.00 HOST_PHYSMEM_GB_OVERRIDE=48 \
        check_host_memory_at_boundary "nav chunk 30/34" >"$OUT/wedge.out" 2>&1
    rc=$?
    assert_eq "1" "$rc" "the wedge reading stops when available is scarce and corroborated"
    assert_says "$OUT/wedge.out" "STOP: available memory 5.00 GB is under the 9.60 GB floor" "the floor is what stopped it"
    assert_says "$OUT/wedge.out" "CORROBORATED SCARCITY" "scarcity is named as the cause"
    assert_silent_about "$OUT/wedge.out" "SURVIVABILITY" "the cap message did not also fire (scarcity is first)"
    assert_silent_about "$OUT/wedge.out" "RUNAWAY BACKSTOP" "the runaway message did not also fire"
}

# ── the in-run floor and the pre-flight gate judge available the same way ────
# Where the gate lives. It is a knowhow script in the ops workspace rather than a
# repo file, so the two agreement tests below locate it and say so loudly when it
# is not on this machine. They never silently pass on a host that has no gate.
PREFLIGHT_GATE="${PREFLIGHT_GATE:-$HOME/workspaces/dev/data/knowhow/lucidos-ops/scripts/preflight-memory-gate.sh}"

# The gate reads the host through vm_stat, sysctl and ps, and it now also SLEEPS
# between its confirm re-samples. Shadow all four on PATH so one known reading
# can be replayed through it, the same way every test above replays one through
# the running guard. awk and sed stay real.
#
# $1 available GB (split evenly over the four buckets the gate sums), $2 the
# pressure level OR a space-separated SEQUENCE of them, one per read with the
# last repeating, $3 swap MB, $4 compressor GB, $5 physical GB.
#
# The sequence is what makes an oscillating host replayable: the gate's own read
# takes the first level and its confirm loop takes the rest. The index lives in a
# file because each read is a separate process.
GATE_PRESSURE_IDX="$SANDBOX/gate-pressure-idx"
GATE_SLEEPS="$SANDBOX/gate-sleeps"
install_gate_host_stubs() {
    local dir="$SANDBOX/gate-bin" pages_per_gb=65536
    mkdir -p "$dir"
    printf '0\n' > "$GATE_PRESSURE_IDX"
    : > "$GATE_SLEEPS"
    cat > "$dir/vm_stat" <<EOF
#!/bin/bash
echo "Mach Virtual Memory Statistics: (page size of 16384 bytes)"
awk -v a="$1" -v c="$4" -v p="$pages_per_gb" 'BEGIN {
    printf "Pages free:                          %d.\n", a * p / 4
    printf "Pages speculative:                   %d.\n", a * p / 4
    printf "Pages purgeable:                     %d.\n", a * p / 4
    printf "File-backed pages:                   %d.\n", a * p / 4
    printf "Pages occupied by compressor:        %d.\n", c * p
    printf "Anonymous pages:                     %d.\n", p
}'
EOF
    cat > "$dir/sysctl" <<EOF
#!/bin/bash
case "\$*" in
    *kern.memorystatus_vm_pressure_level*)
        i="\$(cat "$GATE_PRESSURE_IDX" 2>/dev/null || echo 0)"
        echo "\$((i + 1))" > "$GATE_PRESSURE_IDX"
        awk -v s="$2" -v i="\$i" 'BEGIN {
            n = split(s, a, " ")
            print (i + 1 <= n) ? a[i + 1] : a[n]
        }'
        ;;
    *vm.swapusage*) echo "total = 0.00M  used = $3M  free = 0.00M  (encrypted)" ;;
    *hw.memsize*) awk -v g="$5" 'BEGIN { printf "%.0f\n", g * 1073741824 }' ;;
    *) exit 1 ;;
esac
EOF
    # The gate's confirm window is 40 seconds of real sleeping. Record what it
    # was asked to wait instead, so the suite stays instant and "did it wait at
    # all?" is still assertable: an arm that must be immediate must not sleep.
    cat > "$dir/sleep" <<EOF
#!/bin/bash
printf '%s\n' "\$1" >> "$GATE_SLEEPS"
exit 0
EOF
    printf '#!/bin/bash\nexit 0\n' > "$dir/ps"
    chmod +x "$dir/vm_stat" "$dir/sysctl" "$dir/ps" "$dir/sleep"
    printf '%s' "$dir"
}

gate_sleep_count() { wc -l < "$GATE_SLEEPS" | tr -d ' '; }

# The 8 GB minimum is the number both guards share, so it is pinned on both sides
# rather than only on ours. The gate's own thresholds are read out of its source,
# because a comment claiming they match is not a test.
test_the_floor_is_never_below_the_preflight_gate() {
    echo "test: the in-run floor is never below the pre-flight gate's 8 GB threshold"
    local got size
    reset_state
    assert_eq "8" "$HOST_MEMORY_FREE_FLOOR_ABS_GB" "the absolute floor matches the gate's AVAILABLE_MIN_GB"
    for size in 1 8 16 48 96; do
        got="$(HOST_PHYSMEM_GB_OVERRIDE="$size" _host_mem_available_floor_gb)"
        if awk -v v="$got" 'BEGIN { exit (v + 0 >= 8) ? 0 : 1 }'; then
            pass "floor on a ${size} GB host is $got GB, at or above 8"
        else
            fail "floor on a ${size} GB host is $got GB, below the gate's 8 GB"
        fi
    done

    if [ ! -f "$PREFLIGHT_GATE" ]; then
        skip "the pre-flight gate is not on this machine ($PREFLIGHT_GATE), so its half is unverified here"
        return
    fi
    if grep -qE '^AVAILABLE_MIN_GB=\$\{AVAILABLE_MIN_GB:-8\}' "$PREFLIGHT_GATE"; then
        pass "the gate's AVAILABLE_MIN_GB default is the same 8 GB"
    else
        fail "the gate's AVAILABLE_MIN_GB default is no longer 8 GB"
    fi
    if grep -q 'corroborat' "$PREFLIGHT_GATE"; then
        pass "the gate's available line is corroborated, like the in-run floor"
    else
        fail "the gate's available line lost its corroboration"
    fi
    if grep -qE '^COMPRESSOR_MAX_GB=' "$PREFLIGHT_GATE"; then
        fail "the gate's unconditional COMPRESSOR_MAX_GB is back; it was retired for a runaway backstop"
    else
        pass "the gate has no unconditional compressor ceiling"
    fi

    # The critical-pressure rule is the third instrument the two share, and every
    # number in it is pinned on both sides. A comment claiming they match is not
    # a test, so each default is read out of the gate's own source.
    if grep -qE '^PRESSURE_MAX=' "$PREFLIGHT_GATE"; then
        fail "the gate's unconditional PRESSURE_MAX is back; it made warn a refusal, which the guard has never done"
    else
        pass "the gate has no unconditional pressure ceiling"
    fi
    assert_eq "2" "$HOST_MEMORY_COLLAPSE_ABS_GB" "the guard's collapse minimum is 2 GB"
    assert_eq "5" "$HOST_MEMORY_COLLAPSE_PCT" "the guard's collapse share is 5% of RAM"
    assert_eq "8" "$HOST_MEMORY_CRITICAL_CONFIRM_SAMPLES" "the guard confirms over 8 samples"
    assert_eq "5" "$HOST_MEMORY_CRITICAL_CONFIRM_SECS" "the guard spaces them 5 s apart"
    for pin in 'COLLAPSE_MIN_GB=\$\{COLLAPSE_MIN_GB:-2\}' \
        'COLLAPSE_PCT=\$\{COLLAPSE_PCT:-5\}' \
        'CRITICAL_CONFIRM_SAMPLES=\$\{CRITICAL_CONFIRM_SAMPLES:-8\}' \
        'CRITICAL_CONFIRM_SECS=\$\{CRITICAL_CONFIRM_SECS:-5\}'; do
        if grep -qE "^$pin" "$PREFLIGHT_GATE"; then
            pass "the gate carries the guard's own default: ${pin%%=*}"
        else
            fail "the gate's ${pin%%=*} no longer matches the guard's value"
        fi
    done
}

# THE 2026-09-12 OSCILLATION TRACE, replayed through both instruments. Ten
# samples of an idle host 18 seconds apart, nothing of ours running: the level
# goes 2 2 4 4 2 4 4 2 4 4 while available stays flat at 11 GB, the compressor
# does not move by a page, swap is exactly zero and load is falling. Six of ten
# read critical. This is the reading that refused 22 gate checks and three
# nightlies, and neither instrument may act on it.
test_the_oscillating_idle_host_stops_neither_guard() {
    echo "test: the idle-host pressure oscillation (2/4 flapping, available flat at 11 GB) is GO for both"
    local rc bin
    reset_state
    # The trace as the sampler would have recorded it: level, compressor,
    # available, swap. Every available reading is above the 9.60 GB floor, so the
    # floor is not what is under test here; the pressure column is.
    printf '%s\n' \
        '2 17.28 11.08 0.00' '2 17.28 11.03 0.00' '4 17.28 10.84 0.00' \
        '4 17.28 11.05 0.00' '2 17.28 11.09 0.00' '4 17.28 11.07 0.00' \
        '4 17.28 11.09 0.00' '2 17.28 11.10 0.00' '4 17.28 11.00 0.00' \
        '4 17.28 11.01 0.00' > "$HOST_MEMORY_SAMPLES_FILE"
    (
        # The boundary lands on a critical reading, and the flapping continues
        # into the confirm exactly as the trace does.
        use_pressure_sequence 4 4 2 4 4 2
        HOST_COMPRESSOR_GB_OVERRIDE=17.28 HOST_AVAIL_GB_OVERRIDE=11.01 \
            HOST_SWAP_USED_GB_OVERRIDE=0.00 HOST_PHYSMEM_GB_OVERRIDE=48 \
            check_host_memory_at_boundary "nav chunk 33/34"
    ) >"$OUT/osc.out" 2>&1
    rc=$?
    assert_eq "0" "$rc" "the running guard does not stop on the oscillating idle host"
    assert_silent_about "$OUT/osc.out" "STOP:" "the running guard announced no stop"
    assert_says "$OUT/osc.out" "did not hold it" "the running guard names the reading as transient"
    assert_says "$OUT/osc.out" "worst of 10 samples" "it judged the whole trace"

    if [ ! -f "$PREFLIGHT_GATE" ]; then
        skip "the pre-flight gate is not on this machine ($PREFLIGHT_GATE), so its half of the agreement is unverified here"
        return
    fi
    bin="$(install_gate_host_stubs 11.01 "4 4 2 4 4 2" 0.00 17.28 48)"
    PATH="$bin:$PATH" bash "$PREFLIGHT_GATE" >"$OUT/osc-gate.out" 2>&1
    rc=$?
    assert_eq "0" "$rc" "the pre-flight gate says GO on the same oscillation"
    assert_says "$OUT/osc-gate.out" "VERDICT: GO" "the gate's verdict is GO"
    assert_silent_about "$OUT/osc-gate.out" "NO-GO" "the gate refused nothing"
    assert_says "$OUT/osc-gate.out" "CRITICAL did not hold" "the gate names the reading as transient"
}

# The other end of the same instrument, and the reading that must never stop
# being a stop. Critical with the headroom gone is the 2026-07-26 freeze, and
# neither instrument may wait 40 seconds before getting out of the way.
test_the_freeze_reading_stops_both_guards() {
    echo "test: the freeze reading (critical, 0.30 GB available) stops the guard and refuses the gate, at once"
    local rc bin
    reset_state
    HOST_PRESSURE_LEVEL_OVERRIDE=4 HOST_COMPRESSOR_GB_OVERRIDE=17.41 \
        HOST_AVAIL_GB_OVERRIDE=0.30 HOST_SWAP_USED_GB_OVERRIDE=0.00 \
        HOST_PHYSMEM_GB_OVERRIDE=48 \
        check_host_memory_at_boundary "nav chunk 33/34" >"$OUT/freezeboth.out" 2>&1
    rc=$?
    assert_eq "1" "$rc" "the running guard stops on the freeze signature"
    assert_says "$OUT/freezeboth.out" "FREEZE SIGNATURE" "the running guard names the arm"
    assert_eq "0" "$(confirm_wait_count)" "the running guard did not wait"

    if [ ! -f "$PREFLIGHT_GATE" ]; then
        skip "the pre-flight gate is not on this machine ($PREFLIGHT_GATE), so its half of the agreement is unverified here"
        return
    fi
    bin="$(install_gate_host_stubs 0.30 4 0.00 17.41 48)"
    PATH="$bin:$PATH" bash "$PREFLIGHT_GATE" >"$OUT/freeze-gate.out" 2>&1
    rc=$?
    assert_eq "1" "$rc" "the pre-flight gate refuses the freeze reading"
    assert_says "$OUT/freeze-gate.out" "VERDICT: NO-GO" "the gate's verdict is NO-GO"
    assert_says "$OUT/freeze-gate.out" "freeze signature" "the gate names what it saw"
    assert_eq "0" "$(gate_sleep_count)" "the gate did not wait either"
}

# And the middle case, which is the new rule working. Nothing corroborates, so
# both instruments spend the whole window, and both then act.
test_sustained_critical_stops_both_guards() {
    echo "test: critical that holds through the whole window stops the guard and refuses the gate"
    local rc bin
    reset_state
    HOST_PRESSURE_LEVEL_OVERRIDE=4 HOST_COMPRESSOR_GB_OVERRIDE=17.28 \
        HOST_AVAIL_GB_OVERRIDE=11.00 HOST_SWAP_USED_GB_OVERRIDE=0.00 \
        HOST_PHYSMEM_GB_OVERRIDE=48 \
        check_host_memory_at_boundary "nav chunk 33/34" >"$OUT/heldboth.out" 2>&1
    rc=$?
    assert_eq "1" "$rc" "the running guard stops once critical has held"
    assert_says "$OUT/heldboth.out" "and it HELD" "the running guard says the level persisted"
    assert_eq "8" "$(confirm_wait_count)" "the running guard spent the whole window"

    if [ ! -f "$PREFLIGHT_GATE" ]; then
        skip "the pre-flight gate is not on this machine ($PREFLIGHT_GATE), so its half of the agreement is unverified here"
        return
    fi
    bin="$(install_gate_host_stubs 11.00 4 0.00 17.28 48)"
    PATH="$bin:$PATH" bash "$PREFLIGHT_GATE" >"$OUT/held-gate.out" 2>&1
    rc=$?
    assert_eq "1" "$rc" "the pre-flight gate refuses a sustained critical"
    assert_says "$OUT/held-gate.out" "VERDICT: NO-GO" "the gate's verdict is NO-GO"
    assert_says "$OUT/held-gate.out" "held through all 8 re-samples over 40s" "the gate quotes the window it spent"
    assert_eq "8" "$(gate_sleep_count)" "the gate spent the whole window too"
}

# THE INVARIANT THAT KEEPS THIS BUG FROM COMING BACK IN A THIRD FORM. A reading a
# completely idle host produces must never be able to stop anything, and the two
# guards must agree about it. This is the 09:00 probe of 2026-09-09: nothing was
# running, and the old floor and the old gate would both have refused the host.
test_an_idle_host_reading_stops_neither_guard() {
    echo "test: the idle-host reading (available 8.79, pressure normal, swap 0, compressor 14.30) is GO for both"
    local rc bin
    reset_state
    HOST_PRESSURE_LEVEL_OVERRIDE=1 HOST_COMPRESSOR_GB_OVERRIDE=14.30 \
        HOST_AVAIL_GB_OVERRIDE=8.79 HOST_SWAP_USED_GB_OVERRIDE=0.00 \
        HOST_PHYSMEM_GB_OVERRIDE=48 \
        check_host_memory_at_boundary "nav chunk 21/34" >"$OUT/idle.out" 2>&1
    rc=$?
    assert_eq "0" "$rc" "the running guard does not stop on an idle host's reading"
    assert_silent_about "$OUT/idle.out" "STOP:" "the running guard announced no stop"
    assert_says "$OUT/idle.out" "it is uncorroborated" "the running guard records the reading and says why it declined"

    if [ ! -f "$PREFLIGHT_GATE" ]; then
        skip "the pre-flight gate is not on this machine ($PREFLIGHT_GATE), so its half of the agreement is unverified here"
        return
    fi
    bin="$(install_gate_host_stubs 8.79 1 0.00 14.30 48)"
    PATH="$bin:$PATH" bash "$PREFLIGHT_GATE" >"$OUT/idle-gate.out" 2>&1
    rc=$?
    assert_eq "0" "$rc" "the pre-flight gate says GO on the same idle-host reading"
    assert_says "$OUT/idle-gate.out" "VERDICT: GO" "the gate's verdict is GO"
    assert_silent_about "$OUT/idle-gate.out" "NO-GO" "the gate refused nothing"
}

# The other half of the agreement: the two must also both stop on a host that is
# genuinely in trouble. Available under the floor with swap in use corroborates
# on both sides, so neither can drift into ignoring a real one.
test_a_corroborated_low_host_stops_both_guards() {
    echo "test: available 3.00 GB with swap in use is a stop for the guard and a NO-GO for the gate"
    local rc bin
    reset_state
    HOST_PRESSURE_LEVEL_OVERRIDE=1 HOST_COMPRESSOR_GB_OVERRIDE=10.00 \
        HOST_AVAIL_GB_OVERRIDE=3.00 HOST_SWAP_USED_GB_OVERRIDE=0.25 \
        HOST_PHYSMEM_GB_OVERRIDE=48 \
        check_host_memory_at_boundary "nav chunk 22/34" >"$OUT/lowboth.out" 2>&1
    rc=$?
    assert_eq "1" "$rc" "the running guard stops on corroborated scarcity"
    assert_says "$OUT/lowboth.out" "CORROBORATED SCARCITY" "the running guard names the stop"

    if [ ! -f "$PREFLIGHT_GATE" ]; then
        skip "the pre-flight gate is not on this machine ($PREFLIGHT_GATE), so its half of the agreement is unverified here"
        return
    fi
    bin="$(install_gate_host_stubs 3.00 1 256.00 10.00 48)"
    PATH="$bin:$PATH" bash "$PREFLIGHT_GATE" >"$OUT/low-gate.out" 2>&1
    rc=$?
    assert_eq "1" "$rc" "the pre-flight gate refuses the same host"
    assert_says "$OUT/low-gate.out" "VERDICT: NO-GO" "the gate's verdict is NO-GO"
    assert_says "$OUT/low-gate.out" "corroborated" "the gate names the corroboration it found"
}

# The gate's half of the knob invariant the guard pins just below. Its two
# confirm knobs are the only ones that fail DANGEROUSLY: a non-numeric count
# makes its `-lt` test error, so the loop never runs and one reading becomes a
# NO-GO, and a zero interval takes every sample in the same instant. Both are
# the false stop this whole rule exists to remove, so both must fall back.
test_a_garbage_confirm_knob_does_not_refuse_the_gate() {
    echo "test: an unusable confirm knob falls back rather than refusing a healthy host"
    local rc bin
    reset_state
    if [ ! -f "$PREFLIGHT_GATE" ]; then
        skip "the pre-flight gate is not on this machine ($PREFLIGHT_GATE), so its knobs are unverified here"
        return
    fi
    bin="$(install_gate_host_stubs 11.01 "4 2" 0.00 17.28 48)"
    PATH="$bin:$PATH" CRITICAL_CONFIRM_SAMPLES=banana bash "$PREFLIGHT_GATE" >"$OUT/badknob.out" 2>&1
    rc=$?
    assert_eq "0" "$rc" "a non-numeric sample count does not refuse the host"
    assert_says "$OUT/badknob.out" "is not a count of 2 or more, using 8" "the gate names the value it rejected"
    assert_says "$OUT/badknob.out" "VERDICT: GO" "the gate still judged the host on its readings"

    bin="$(install_gate_host_stubs 11.01 "4 2" 0.00 17.28 48)"
    PATH="$bin:$PATH" CRITICAL_CONFIRM_SECS=0 bash "$PREFLIGHT_GATE" >"$OUT/badsecs.out" 2>&1
    rc=$?
    assert_eq "0" "$rc" "a zero interval does not refuse the host"
    assert_says "$OUT/badsecs.out" "is not a wait of a second or more, using 5" "the gate names the interval it rejected"

    # `1e3` is the one the guard's own shape test exists for: awk reads it as
    # 1000, and `sleep 1e3` then errors and waits for nothing. The two
    # instruments reject it identically.
    assert_eq "5" "$(LUCIDOS_E2E_CRITICAL_SECS=1e3 _host_mem_critical_confirm_secs)" "the guard rejects an exponent"
    bin="$(install_gate_host_stubs 11.01 "4 2" 0.00 17.28 48)"
    PATH="$bin:$PATH" CRITICAL_CONFIRM_SECS=1e3 bash "$PREFLIGHT_GATE" >"$OUT/expsecs.out" 2>&1
    rc=$?
    assert_eq "0" "$rc" "the gate rejects an exponent too, rather than waiting for nothing"
    assert_says "$OUT/expsecs.out" "CRITICAL_CONFIRM_SECS='1e3'" "the gate names the exponent it rejected"

    # The retired knob is named rather than silently obeyed, the same treatment
    # the guard gives LUCIDOS_E2E_COMPRESSOR_CAP_GB.
    bin="$(install_gate_host_stubs 11.01 "4 2" 0.00 17.28 48)"
    PATH="$bin:$PATH" PRESSURE_MAX=1 bash "$PREFLIGHT_GATE" >"$OUT/staleknob.out" 2>&1
    rc=$?
    assert_eq "0" "$rc" "a stale PRESSURE_MAX no longer refuses a host at warn"
    assert_says "$OUT/staleknob.out" "PRESSURE_MAX is set and is no longer read" "the retired knob is named"
}

# ── Test 4: the runaway backstop, the only compressor ceiling left ───────────
test_the_runaway_backstop_scales_with_ram() {
    echo "test: the runaway is 50% of RAM, and no lower compressor ceiling exists"
    local rc got
    reset_state
    got="$(HOST_PHYSMEM_GB_OVERRIDE=48 _host_mem_compressor_ceiling_gb)"
    assert_eq "24.00" "$got" "48 GB runaway resolves to 50% of RAM"
    got="$(HOST_PHYSMEM_GB_OVERRIDE=16 _host_mem_compressor_ceiling_gb)"
    assert_eq "8.00" "$got" "16 GB runaway resolves to 50% of RAM"

    # The retired survivability cap. Its resolver is gone, so a reader that comes
    # back is a revert, and the assertion names it rather than leaving a hole.
    if command -v _host_mem_compressor_cap_gb >/dev/null 2>&1; then
        fail "_host_mem_compressor_cap_gb is back; the survivability cap was retired"
    else
        pass "the survivability cap has no resolver, so nothing below the runaway can stop"
    fi

    # A compressor above the retired cap but below the runaway, on a healthy host,
    # now runs on. That is the whole point of the change.
    HOST_COMPRESSOR_GB_OVERRIDE=17.32 HOST_AVAIL_GB_OVERRIDE=20.00 \
        HOST_SWAP_USED_GB_OVERRIDE=0.00 HOST_PHYSMEM_GB_OVERRIDE=48 \
        check_host_memory_at_boundary "nav chunk 18/34" >"$OUT/nocap.out" 2>&1
    rc=$?
    assert_eq "0" "$rc" "17.32 GB compressed on a healthy 48 GB host runs on"
    assert_silent_about "$OUT/nocap.out" "STOP" "no stop was announced"

    # Over the runaway, on the same healthy host, it stops. The backstop is the
    # one compressor ceiling left.
    reset_state
    HOST_COMPRESSOR_GB_OVERRIDE=25.00 HOST_AVAIL_GB_OVERRIDE=20.00 \
        HOST_SWAP_USED_GB_OVERRIDE=0.00 HOST_PHYSMEM_GB_OVERRIDE=48 \
        check_host_memory_at_boundary "nav chunk 30/34" >"$OUT/runaway.out" 2>&1
    rc=$?
    assert_eq "1" "$rc" "25 GB compressed stops on the runaway"
    assert_says "$OUT/runaway.out" "over the 24.00 GB backstop" "the runaway is half of RAM"
    assert_says "$OUT/runaway.out" "RUNAWAY BACKSTOP" "the runaway names itself"
    assert_silent_about "$OUT/runaway.out" "SURVIVABILITY" "no survivability cap wording survives"
}

# ── Test 5: the kernel's own verdict is the danger stop, ONCE IT HOLDS ───────
# CRITICAL is the freeze signature: the one recorded freeze on this host read
# compressor 17.41 GB, free 0.04 GB, pressure critical. Compressor size did not
# distinguish it from a healthy night; pressure did. What pressure alone does NOT
# distinguish is an idle host: it read critical in six of ten samples at 15:52 to
# 15:55 on 2026-09-12 with available flat at 11 GB. So it is re-sampled first.
test_critical_pressure_stops_an_otherwise_healthy_looking_host() {
    echo "test: kernel pressure critical that HOLDS stops, even with every other reading fine"
    local rc
    reset_state
    HOST_PRESSURE_LEVEL_OVERRIDE=4 HOST_COMPRESSOR_GB_OVERRIDE=6.00 \
        HOST_AVAIL_GB_OVERRIDE=30.00 HOST_SWAP_USED_GB_OVERRIDE=0.00 \
        HOST_PHYSMEM_GB_OVERRIDE=48 \
        check_host_memory_at_boundary "nav chunk 9/34" >"$OUT/crit.out" 2>&1
    rc=$?
    assert_eq "1" "$rc" "sustained critical pressure stops the run"
    assert_says "$OUT/crit.out" "CRITICAL memory pressure" "the stop names the pressure level"
    assert_says "$OUT/crit.out" "KERNEL'S OWN VERDICT" "the stop says why pressure is the instrument"
    assert_says "$OUT/crit.out" "pressure critical," "the boundary line states the level"
    assert_silent_about "$OUT/crit.out" "RUNAWAY" "the compressor did not also fire"
    # It was not believed on the first reading. Nothing corroborated it here, so
    # the whole window had to be spent before the stop was allowed.
    assert_says "$OUT/crit.out" "8 of 8 samples over" "the stop quotes the confirm window it spent"
    assert_eq "8" "$(confirm_wait_count)" "the confirm took all eight samples"
    assert_eq "5" "$(confirm_wait_intervals)" "the samples were five seconds apart"
}

# THE REGRESSION THIS CHANGE EXISTS TO REMOVE. One critical reading at the
# boundary, on a host whose every other instrument says it is fine, used to end
# the run at once. On the measured host that reading arrives six times in ten
# samples with nothing running.
test_a_single_critical_reading_at_the_boundary_does_not_stop() {
    echo "test: a critical boundary reading that does not hold is recorded, not acted on"
    local rc
    reset_state
    (
        # Critical at the boundary, then the level drops back on the very first
        # confirm sample. That is the oscillation, replayed.
        use_pressure_sequence 4 2 4 2
        HOST_COMPRESSOR_GB_OVERRIDE=17.28 HOST_AVAIL_GB_OVERRIDE=11.01 \
            HOST_SWAP_USED_GB_OVERRIDE=0.00 HOST_PHYSMEM_GB_OVERRIDE=48 \
            check_host_memory_at_boundary "nav chunk 9/34"
    ) >"$OUT/crittick.out" 2>&1
    rc=$?
    assert_eq "0" "$rc" "one critical reading does not stop the run"
    assert_silent_about "$OUT/crittick.out" "STOP:" "no stop was announced"
    assert_says "$OUT/crittick.out" "did not hold it" "the note says the level did not hold"
    assert_says "$OUT/crittick.out" "Sample 1 of 8" "the note says which sample ended the confirm"
    assert_says "$OUT/crittick.out" "read warn" "the note names the level that ended it"
    assert_says "$OUT/crittick.out" "transient edge notification" "the note names what the reading is"
    assert_says "$OUT/crittick.out" "or available at or under 2.40 GB, or swap in use" "the note states what a stop would have needed"
    assert_eq "1" "$(confirm_wait_count)" "the confirm exited on the first non-critical sample"
}

# The freeze arm. Critical WITH the headroom gone is unconditional: it must not
# spend 40 seconds confirming a host that is already wedging.
test_critical_with_the_headroom_gone_stops_without_waiting() {
    echo "test: critical with available under the collapse level stops at once"
    local rc
    reset_state
    HOST_PRESSURE_LEVEL_OVERRIDE=4 HOST_COMPRESSOR_GB_OVERRIDE=17.41 \
        HOST_AVAIL_GB_OVERRIDE=1.50 HOST_SWAP_USED_GB_OVERRIDE=0.00 \
        HOST_PHYSMEM_GB_OVERRIDE=48 \
        check_host_memory_at_boundary "nav chunk 9/34" >"$OUT/collapse.out" 2>&1
    rc=$?
    assert_eq "1" "$rc" "the collapse arm stops the run"
    assert_says "$OUT/collapse.out" "FREEZE SIGNATURE" "the stop names itself"
    assert_says "$OUT/collapse.out" "at or under the 2.40 GB collapse level" "the stop quotes the level it broke"
    assert_eq "0" "$(confirm_wait_count)" "it waited for nothing"
}

# The swap arm. Swap is accumulated state, so it corroborates from the first byte
# and needs no window either.
test_critical_with_swap_in_use_stops_without_waiting() {
    echo "test: critical with any swap in use stops at once"
    local rc
    reset_state
    HOST_PRESSURE_LEVEL_OVERRIDE=4 HOST_COMPRESSOR_GB_OVERRIDE=9.00 \
        HOST_AVAIL_GB_OVERRIDE=30.00 HOST_SWAP_USED_GB_OVERRIDE=0.25 \
        HOST_PHYSMEM_GB_OVERRIDE=48 \
        check_host_memory_at_boundary "nav chunk 9/34" >"$OUT/critswap.out" 2>&1
    rc=$?
    assert_eq "1" "$rc" "the swap-corroborated arm stops the run"
    assert_says "$OUT/critswap.out" "CORROBORATED CRITICAL PRESSURE" "the stop names itself"
    assert_says "$OUT/critswap.out" "0.25 GB of" "the stop quotes the swap reading"
    assert_eq "0" "$(confirm_wait_count)" "it waited for nothing"
    # 0.25 GB is under the 1 GB swap ceiling, so the swap stop of its own could
    # not have fired here. Only the conjunction did.
    assert_silent_about "$OUT/critswap.out" "MEASURED DISTRESS" "the swap stop of its own did not fire"
}

# Fail open, mid-confirm. An oid that stops answering is not evidence the host is
# in trouble, so the confirm ends and the run continues.
test_an_unreadable_level_mid_confirm_does_not_stop() {
    echo "test: an unreadable level during the confirm ends it without a stop"
    local rc
    reset_state
    (
        use_pressure_sequence 4 4 4 x
        HOST_COMPRESSOR_GB_OVERRIDE=6.00 HOST_AVAIL_GB_OVERRIDE=30.00 \
            HOST_SWAP_USED_GB_OVERRIDE=0.00 HOST_PHYSMEM_GB_OVERRIDE=48 \
            check_host_memory_at_boundary "nav chunk 9/34"
    ) >"$OUT/critblind.out" 2>&1
    rc=$?
    assert_eq "0" "$rc" "an unreadable level mid-confirm does not stop the run"
    assert_silent_about "$OUT/critblind.out" "STOP:" "no stop was announced"
    assert_says "$OUT/critblind.out" "read unreadable" "the note names the reading that ended the confirm"
    assert_says "$OUT/critblind.out" "Sample 3 of 8" "it ended on the sample that could not be read"
}

# The window's own knobs. A garbage value must fall back, and so must a value
# that would disarm the rule: one sample is not a window, and a zero interval
# takes every reading of the same latched level in the same instant.
test_the_confirm_window_knobs_fall_back_rather_than_disarming() {
    echo "test: the confirm knobs reject a value that would disarm the sustain rule"
    assert_eq "8" "$(_host_mem_critical_confirm_samples)" "the default is eight samples"
    assert_eq "5" "$(_host_mem_critical_confirm_secs)" "the default is five seconds apart"
    assert_eq "4" "$(LUCIDOS_E2E_CRITICAL_SAMPLES=4 _host_mem_critical_confirm_samples)" "an explicit count is honoured"
    assert_eq "8" "$(LUCIDOS_E2E_CRITICAL_SAMPLES=1 _host_mem_critical_confirm_samples)" "one sample is not a window, so it falls back"
    assert_eq "8" "$(LUCIDOS_E2E_CRITICAL_SAMPLES=banana _host_mem_critical_confirm_samples)" "a non-numeric count falls back"
    assert_eq "8" "$(LUCIDOS_E2E_CRITICAL_SAMPLES=08 _host_mem_critical_confirm_samples)" "a leading zero is read as decimal, not octal"
    assert_eq "5" "$(LUCIDOS_E2E_CRITICAL_SECS=0 _host_mem_critical_confirm_secs)" "a zero interval falls back"
    assert_eq "5" "$(LUCIDOS_E2E_CRITICAL_SECS=banana _host_mem_critical_confirm_secs)" "a non-numeric interval falls back"
    assert_eq "10" "$(LUCIDOS_E2E_CRITICAL_SECS=10 _host_mem_critical_confirm_secs)" "an explicit interval is honoured"
}

# The collapse level scales with the host the way the floor does, and sits well
# under the lowest reading an idle host has produced (8.79 GB).
test_the_collapse_level_scales_with_ram() {
    echo "test: the collapse level is max(2 GB, 5% of RAM) and never reaches an idle host"
    local got size
    assert_eq "2.40" "$(HOST_PHYSMEM_GB_OVERRIDE=48 _host_mem_collapse_level_gb)" "2.40 GB on a 48 GB host"
    assert_eq "2.00" "$(HOST_PHYSMEM_GB_OVERRIDE=16 _host_mem_collapse_level_gb)" "the 2 GB minimum holds on a 16 GB host"
    assert_eq "4.80" "$(HOST_PHYSMEM_GB_OVERRIDE=96 _host_mem_collapse_level_gb)" "4.80 GB on a 96 GB host"
    # Shadow sysctl in a subshell so hw.memsize is unreadable. A level in bytes
    # needs no RAM total, so the absolute still applies rather than lapsing.
    # shellcheck disable=SC2329 # a seam: invoked by the sourced guard, not from this file
    assert_eq "2" "$(sysctl() { return 1; }; _host_mem_collapse_level_gb)" "unreadable RAM leaves the absolute standing"
    assert_eq "2.40" "$(HOST_PHYSMEM_GB_OVERRIDE=48 LUCIDOS_E2E_COLLAPSE_PCT=banana _host_mem_collapse_level_gb)" "a garbage share falls back"
    # The collapse level must stay under the available floor on every host size,
    # or critical pressure would stop before the corroborated floor ever could.
    for size in 1 8 16 48 96; do
        got="$(HOST_PHYSMEM_GB_OVERRIDE="$size" _host_mem_collapse_level_gb)"
        if awk -v c="$got" -v f="$(HOST_PHYSMEM_GB_OVERRIDE="$size" _host_mem_available_floor_gb)" \
            'BEGIN { exit (c + 0 < f + 0) ? 0 : 1 }'; then
            pass "collapse on a ${size} GB host is $got GB, under that host's floor"
        else
            fail "collapse on a ${size} GB host is $got GB, at or above its floor"
        fi
    done
}

# WARN is reported and never acted on: 92 occurrences across 5749 host samples
# with no freeze, and it tracks host load as much as memory.
test_warn_pressure_is_reported_but_never_stops() {
    echo "test: kernel pressure warn is stated and the run continues"
    local rc
    reset_state
    HOST_PRESSURE_LEVEL_OVERRIDE=2 HOST_COMPRESSOR_GB_OVERRIDE=16.63 \
        HOST_AVAIL_GB_OVERRIDE=20.00 HOST_SWAP_USED_GB_OVERRIDE=0.00 \
        HOST_PHYSMEM_GB_OVERRIDE=48 \
        check_host_memory_at_boundary "nav chunk 12/34" >"$OUT/warn.out" 2>&1
    rc=$?
    assert_eq "0" "$rc" "warn pressure does not stop the run"
    assert_says "$OUT/warn.out" "WARN pressure" "the warn is stated"
    assert_says "$OUT/warn.out" "not a stop" "the line says it is not a stop"
    assert_silent_about "$OUT/warn.out" "STOP:" "no stop was announced"
}

# Fail open, the same posture every other reader here has. An unreadable oid must
# never be able to end a run.
test_unreadable_pressure_never_stops() {
    echo "test: unreadable pressure runs on and reports itself as unreadable"
    local rc
    reset_state
    # The file-wide pin has to come OFF for this one, and a bare empty assignment
    # would not do it: the reader treats empty as absent and falls through to the
    # real sysctl, so the test would silently read the CI host. Subshell, so the
    # unset cannot leak forward. Every other dimension is injected, so the stubbed
    # sysctl reaches nothing but the pressure oid.
    (
        unset HOST_PRESSURE_LEVEL_OVERRIDE
        # shellcheck disable=SC2329 # a seam: invoked by the sourced guard, not from this file
        sysctl() { return 1; }
        HOST_COMPRESSOR_GB_OVERRIDE=12.00 HOST_AVAIL_GB_OVERRIDE=30.00 \
            HOST_SWAP_USED_GB_OVERRIDE=0.00 HOST_PHYSMEM_GB_OVERRIDE=48 \
            check_host_memory_at_boundary "nav chunk 3/34"
    ) >"$OUT/nopress.out" 2>&1
    rc=$?
    assert_eq "0" "$rc" "an unreadable pressure level does not stop the run"
    assert_says "$OUT/nopress.out" "pressure unreadable" "the line says the level could not be read"
}

# A sysctl that answers with something that is not a level must not be believed.
# The reader returns nothing, so the guard behaves exactly as if the oid were
# missing rather than treating a stray value as normal or as critical.
test_a_non_level_pressure_value_is_discarded() {
    echo "test: a pressure value outside 1/2/4 is discarded, not guessed at"
    local got
    reset_state
    got="$(HOST_PRESSURE_LEVEL_OVERRIDE=1 _host_mem_read_pressure_level)"
    assert_eq "1" "$got" "a real level passes through"
    got="$(_host_mem_pressure_word 4)"
    assert_eq "critical" "$got" "level 4 reads as critical"
    got="$(_host_mem_pressure_word "")"
    assert_eq "unreadable" "$got" "an absent level reads as unreadable, never as normal"
    got="$(sysctl() { return 1; }; unset HOST_PRESSURE_LEVEL_OVERRIDE; _host_mem_read_pressure_level)"
    assert_eq "" "$got" "a failing sysctl yields nothing"
    got="$(sysctl() { echo 7; }; unset HOST_PRESSURE_LEVEL_OVERRIDE; _host_mem_read_pressure_level)"
    assert_eq "" "$got" "a value that is not a level yields nothing"
}

# ── Test 6: last night's stop, which is the regression this change removes ───
# The exact boundary that ended nav chunk 33 of 34: compressor 17.11 GB,
# available 13.74 GB, swap 0, pressure normal on a 48 GB host. Every real signal
# said the host was fine and only the retired cap disagreed.
test_last_nights_boundary_now_runs_on() {
    echo "test: 17.11 GB compressor, 13.74 GB available, swap 0, pressure normal runs on"
    local rc
    reset_state
    HOST_PRESSURE_LEVEL_OVERRIDE=1 HOST_COMPRESSOR_GB_OVERRIDE=17.11 \
        HOST_AVAIL_GB_OVERRIDE=13.74 HOST_SWAP_USED_GB_OVERRIDE=0.00 \
        HOST_PHYSMEM_GB_OVERRIDE=48 \
        check_host_memory_at_boundary "nav chunk 33/34" >"$OUT/lastnight.out" 2>&1
    rc=$?
    assert_eq "0" "$rc" "the boundary that lost welcome.spec.ts no longer stops"
    assert_silent_about "$OUT/lastnight.out" "STOP" "no stop was announced"
}

# The false stop this change removes, replayed exactly. Available 9.05 GB against
# the 9.60 GB floor, kernel pressure normal, swap 0.00. Nothing else about the
# host was wrong, and the 55 specs it cut were later discharged clean on the same
# commit with 12.86 GB as their lowest boundary reading.
test_the_available_floor_false_stop_now_runs_on() {
    echo "test: available 9.05 GB with pressure normal and swap 0 runs on"
    local rc
    reset_state
    HOST_PRESSURE_LEVEL_OVERRIDE=1 HOST_COMPRESSOR_GB_OVERRIDE=16.90 \
        HOST_AVAIL_GB_OVERRIDE=9.05 HOST_SWAP_USED_GB_OVERRIDE=0.00 \
        HOST_PHYSMEM_GB_OVERRIDE=48 \
        check_host_memory_at_boundary "nav chunk 16/34" >"$OUT/falsestop.out" 2>&1
    rc=$?
    assert_eq "0" "$rc" "the boundary that stopped four nightlies no longer stops"
    assert_silent_about "$OUT/falsestop.out" "STOP" "no stop was announced"
    assert_says "$OUT/falsestop.out" "it is uncorroborated" "the reading is still recorded, with the reason it was not acted on"
}

# The freeze itself, for contrast, on the same instrument. Compressor 17.41 GB is
# 0.30 GB from the healthy reading above; pressure and available are what differ.
# It must stop INSTANTLY: a host already wedging must not be made to wait out a
# 40-second sustain window before the run gets out of its way.
test_the_recorded_freeze_reading_stops() {
    echo "test: the recorded freeze reading (critical, 0.04 GB free) stops at once"
    local rc
    reset_state
    HOST_PRESSURE_LEVEL_OVERRIDE=4 HOST_COMPRESSOR_GB_OVERRIDE=17.41 \
        HOST_AVAIL_GB_OVERRIDE=0.30 HOST_SWAP_USED_GB_OVERRIDE=0.00 \
        HOST_PHYSMEM_GB_OVERRIDE=48 \
        check_host_memory_at_boundary "nav chunk 33/34" >"$OUT/freeze.out" 2>&1
    rc=$?
    assert_eq "1" "$rc" "the freeze reading stops"
    assert_says "$OUT/freeze.out" "CRITICAL memory pressure" "pressure is what caught it, ahead of the floor"
    assert_says "$OUT/freeze.out" "FREEZE SIGNATURE" "it is the collapse arm that caught it"
    assert_eq "0" "$(confirm_wait_count)" "the freeze signature never waits out a window"
}

# ── Test 7: the boundary judges the whole chunk, not the instant ─────────────
# A boundary sample bounds the reading at the boundary and says nothing about the
# chunk that just ran. The sampler's window is folded over it, worst per
# dimension, so an excursion that came and went is still seen and still reported.
#
# WHAT CHANGED, AND WHY THIS ASSERTION IS INVERTED. This case used to require a
# STOP: two critical samples inside the chunk ended the run even though the
# boundary read normal. The 2026-09-12 trace retired that rule. An idle host
# produced critical in six of ten samples with available flat at 11 GB, so two
# criticals in a window is what a healthy machine looks like here. A critical
# that has cleared by the boundary now leaves nothing live to re-sample, so it is
# reported and never acted on. The collapse and swap arms still read the folded
# window, and the two tests after this one pin them.
test_a_cleared_critical_excursion_is_reported_and_not_acted_on() {
    echo "test: a critical excursion that cleared by the boundary is recorded, not a stop"
    local rc
    reset_state
    printf '1 6.00 30.00 0.00\n4 9.00 11.00 0.00\n4 9.10 11.20 0.00\n' > "$HOST_MEMORY_SAMPLES_FILE"
    HOST_PRESSURE_LEVEL_OVERRIDE=1 HOST_COMPRESSOR_GB_OVERRIDE=6.30 \
        HOST_AVAIL_GB_OVERRIDE=30.00 HOST_SWAP_USED_GB_OVERRIDE=0.00 \
        HOST_PHYSMEM_GB_OVERRIDE=48 \
        check_host_memory_at_boundary "nav chunk 7/34" >"$OUT/peak.out" 2>&1
    rc=$?
    assert_eq "0" "$rc" "the cleared excursion does not stop the run"
    assert_says "$OUT/peak.out" "worst of 3 samples" "the line says it judged the window"
    assert_says "$OUT/peak.out" "pressure critical," "the excursion still reaches the boundary line"
    assert_says "$OUT/peak.out" "CRITICAL in 2 of 3 samples during the chunk" "the note counts the excursion"
    assert_says "$OUT/peak.out" "read normal at the boundary itself" "the note states the boundary reading"
    assert_says "$OUT/peak.out" "had cleared by the boundary" "the note says why it was not acted on"
    assert_silent_about "$OUT/peak.out" "STOP:" "no stop was announced"
    assert_eq "0" "$(confirm_wait_count)" "a cleared excursion is not worth confirming"
}

# The collapse arm still reads the FOLDED window, because a freeze counts
# whenever inside the chunk it happened. Here the boundary itself reads healthy
# on both dimensions and only the window carries the freeze.
test_a_collapse_inside_the_chunk_stops_though_the_boundary_recovered() {
    echo "test: critical with the headroom gone mid-chunk stops, though the boundary recovered"
    local rc
    reset_state
    printf '1 6.00 30.00 0.00\n4 9.00 0.80 0.00\n1 6.10 29.00 0.00\n' > "$HOST_MEMORY_SAMPLES_FILE"
    HOST_PRESSURE_LEVEL_OVERRIDE=1 HOST_COMPRESSOR_GB_OVERRIDE=6.30 \
        HOST_AVAIL_GB_OVERRIDE=30.00 HOST_SWAP_USED_GB_OVERRIDE=0.00 \
        HOST_PHYSMEM_GB_OVERRIDE=48 \
        check_host_memory_at_boundary "nav chunk 7/34" >"$OUT/peakcollapse.out" 2>&1
    rc=$?
    assert_eq "1" "$rc" "the folded freeze signature stops the run"
    assert_says "$OUT/peakcollapse.out" "FREEZE SIGNATURE" "the stop names itself"
    assert_eq "0" "$(confirm_wait_count)" "the collapse arm never waits"
}

test_the_window_folds_worst_per_dimension() {
    echo "test: the window takes the highest pressure, compressor and swap, the lowest available"
    local got
    reset_state
    printf '1 6.00 30.00 0.00\n2 9.50 11.00 0.25\n1 7.00 22.00 0.10\n' > "$HOST_MEMORY_SAMPLES_FILE"
    got="$(_host_mem_window_worst)"
    assert_eq "2 9.50 11.00 0.25 0 1 0 3" "$got" "worst is max level, max compressor, min available, max swap"
    # Reading TRUNCATES, so the next boundary judges its own chunk rather than
    # inheriting this one's peak forever.
    got="$(_host_mem_window_worst)"
    assert_eq "" "$got" "a second read sees an empty window"
}

# The under-floor count is the floor's sustain evidence, and it is the one number
# in the fold that does not move with window length. Without a usable floor there
# is nothing to be under, so the count is zero rather than a guess.
test_the_window_counts_under_floor_samples() {
    echo "test: the fold counts how many samples read under the floor it was given"
    local got
    reset_state
    printf '1 6.00 30.00 0.00\n1 6.50 9.45 0.00\n1 6.20 9.10 0.00\n' > "$HOST_MEMORY_SAMPLES_FILE"
    got="$(_host_mem_window_worst 9.60)"
    assert_eq "1 6.50 9.10 0.00 0 0 2 3" "$got" "two of three samples are under the 9.60 floor"

    reset_state
    printf '1 6.00 30.00 0.00\n1 6.50 9.60 0.00\n' > "$HOST_MEMORY_SAMPLES_FILE"
    got="$(_host_mem_window_worst 9.60)"
    assert_eq "1 6.50 9.60 0.00 0 0 0 2" "$got" "a sample exactly at the floor is not under it"

    reset_state
    printf '1 6.00 3.00 0.00\n1 6.50 2.00 0.00\n' > "$HOST_MEMORY_SAMPLES_FILE"
    got="$(_host_mem_window_worst banana)"
    assert_eq "1 6.50 2.00 0.00 0 0 0 2" "$got" "an unusable floor counts nothing rather than everything"
}

# The corroboration's own sustain evidence, and the counterpart of the critical
# count. A critical sample is warn-or-worse too, so it counts on both; a garbage
# level counts on neither, because a reading nobody can interpret is not evidence.
test_the_window_counts_warn_or_worse_samples() {
    echo "test: the fold counts how many samples read warn or worse"
    local got
    reset_state
    printf '1 6.00 30.00 0.00\n2 6.50 29.00 0.00\n1 6.20 28.00 0.00\n' > "$HOST_MEMORY_SAMPLES_FILE"
    got="$(_host_mem_window_worst)"
    assert_eq "2 6.50 28.00 0.00 0 1 0 3" "$got" "one of three samples read warn"

    reset_state
    printf '4 6.00 30.00 0.00\n2 6.50 29.00 0.00\n' > "$HOST_MEMORY_SAMPLES_FILE"
    got="$(_host_mem_window_worst)"
    assert_eq "4 6.50 29.00 0.00 1 2 0 2" "$got" "a critical sample counts as warn or worse as well"

    reset_state
    printf '7 6.00 30.00 0.00\n- 6.50 29.00 0.00\n' > "$HOST_MEMORY_SAMPLES_FILE"
    got="$(_host_mem_window_worst)"
    assert_eq "7 6.50 29.00 0.00 0 0 0 2" "$got" "a garbage level and an unreadable one corroborate nothing"
}

# A torn final record leaves $3 empty, which is not the "-" sentinel, and awk
# reads "" + 0 as 0. Available folds with MIN, so that zero can never be raised
# again and the boundary stops on a scarcity reading no sample ever took.
test_a_short_record_is_discarded_rather_than_read_as_zero() {
    echo "test: a truncated sample line cannot manufacture a 0.00 GB available reading"
    local got rc
    reset_state
    printf '1 6.00 30.00 0.00\n\n' > "$HOST_MEMORY_SAMPLES_FILE"
    got="$(_host_mem_window_worst)"
    assert_eq "1 6.00 30.00 0.00 0 0 0 1" "$got" "the blank line is dropped, not folded in as zeros"

    reset_state
    printf '1 6.00 30.00 0.00\n1 6.10\n' > "$HOST_MEMORY_SAMPLES_FILE"
    HOST_PRESSURE_LEVEL_OVERRIDE=1 HOST_COMPRESSOR_GB_OVERRIDE=6.10 \
        HOST_AVAIL_GB_OVERRIDE=30.00 HOST_SWAP_USED_GB_OVERRIDE=0.00 \
        HOST_PHYSMEM_GB_OVERRIDE=48 \
        check_host_memory_at_boundary "nav chunk 4/34" >"$OUT/torn.out" 2>&1
    rc=$?
    assert_eq "0" "$rc" "a torn record does not stop a healthy host"
    assert_silent_about "$OUT/torn.out" "REAL SCARCITY" "no scarcity stop was invented"
}

# The window is sampled every 5 s, and the evidence that critical is the freeze
# signature comes from a ten-minute series. One isolated tick is below the
# resolution anything was judged at, so it must not end a run on its own.
test_one_critical_tick_mid_chunk_does_not_stop() {
    echo "test: a single critical sample in the window is reported, not acted on"
    local rc
    reset_state
    printf '1 6.00 30.00 0.00\n4 9.00 20.00 0.00\n1 6.20 29.00 0.00\n' > "$HOST_MEMORY_SAMPLES_FILE"
    HOST_PRESSURE_LEVEL_OVERRIDE=1 HOST_COMPRESSOR_GB_OVERRIDE=6.30 \
        HOST_AVAIL_GB_OVERRIDE=30.00 HOST_SWAP_USED_GB_OVERRIDE=0.00 \
        HOST_PHYSMEM_GB_OVERRIDE=48 \
        check_host_memory_at_boundary "nav chunk 6/34" >"$OUT/onetick.out" 2>&1
    rc=$?
    assert_eq "0" "$rc" "one critical tick does not stop the run"
    assert_says "$OUT/onetick.out" "pressure critical," "the peak is still reported on the line"
    assert_says "$OUT/onetick.out" "CRITICAL in 1 of 3 samples during the chunk" "the note counts the tick it saw"
    assert_silent_about "$OUT/onetick.out" "STOP:" "no stop was announced"
}

# Critical standing at the boundary is the ONLY reading worth re-sampling, and
# re-sampling is what it gets. It no longer stops on its own: the sustain window
# is what turns a reading into evidence.
test_critical_still_standing_at_the_boundary_is_confirmed_before_it_stops() {
    echo "test: critical at the boundary is re-sampled, then stops, with no window at all"
    local rc
    reset_state
    HOST_PRESSURE_LEVEL_OVERRIDE=4 HOST_COMPRESSOR_GB_OVERRIDE=6.00 \
        HOST_AVAIL_GB_OVERRIDE=30.00 HOST_SWAP_USED_GB_OVERRIDE=0.00 \
        HOST_PHYSMEM_GB_OVERRIDE=48 \
        check_host_memory_at_boundary "nav chunk 6/34" >"$OUT/critnow.out" 2>&1
    rc=$?
    assert_eq "1" "$rc" "critical that holds stops without needing a sampler window"
    assert_says "$OUT/critnow.out" "CRITICAL memory pressure" "the stop names the pressure level"
    assert_says "$OUT/critnow.out" "and it HELD" "the stop says the level persisted"
    assert_eq "8" "$(confirm_wait_count)" "the whole confirm window was spent before stopping"
}

# ── The floor needs the same sustain the pressure stop needs (ADR 0176) ──────
# Available folds with MIN, so the window value is the deepest 5-second trough of
# 50 to 120 samples. It falls further the longer the chunk ran. A fresh browser
# per chunk makes exactly such troughs, which is how a healthy host was stopped.
# The nightly stop read 9.45 GB against a 9.60 GB floor, 1.5% under. The boundary
# itself read 13.48 GB, and pressure was normal all night.
test_one_available_dip_mid_chunk_does_not_stop() {
    echo "test: a single sample under the floor mid-chunk is reported, not acted on"
    local rc
    reset_state
    printf '1 6.00 30.00 0.00\n1 6.50 9.45 0.00\n1 6.20 29.00 0.00\n' > "$HOST_MEMORY_SAMPLES_FILE"
    HOST_PRESSURE_LEVEL_OVERRIDE=1 HOST_COMPRESSOR_GB_OVERRIDE=6.30 \
        HOST_AVAIL_GB_OVERRIDE=13.48 HOST_SWAP_USED_GB_OVERRIDE=0.00 \
        HOST_PHYSMEM_GB_OVERRIDE=48 \
        check_host_memory_at_boundary "nav chunk 4/34" >"$OUT/onedip.out" 2>&1
    rc=$?
    assert_eq "0" "$rc" "one 5-second dip under the floor does not stop the run"
    assert_silent_about "$OUT/onedip.out" "STOP:" "no stop was announced"
    assert_silent_about "$OUT/onedip.out" "CORROBORATED SCARCITY" "no scarcity was claimed"
}

# The sustain rule is not weakened by the corroboration, and that is the whole
# point of keeping the two separate. Warn pressure corroborates here, so the
# only thing standing between this reading and a stop is ADR 0176's sustain rule.
test_one_available_dip_under_warn_pressure_still_does_not_stop() {
    echo "test: one dip under the floor with the kernel at warn is still not a stop"
    local rc
    reset_state
    printf '2 6.00 30.00 0.00\n2 6.50 9.45 0.00\n2 6.20 29.00 0.00\n' > "$HOST_MEMORY_SAMPLES_FILE"
    HOST_PRESSURE_LEVEL_OVERRIDE=2 HOST_COMPRESSOR_GB_OVERRIDE=6.30 \
        HOST_AVAIL_GB_OVERRIDE=13.48 HOST_SWAP_USED_GB_OVERRIDE=0.00 \
        HOST_PHYSMEM_GB_OVERRIDE=48 \
        check_host_memory_at_boundary "nav chunk 5/34" >"$OUT/warndip.out" 2>&1
    rc=$?
    assert_eq "0" "$rc" "corroboration does not excuse a single 5-second trough"
    assert_silent_about "$OUT/warndip.out" "STOP:" "no stop was announced"
    assert_says "$OUT/warndip.out" "note: available dipped to 9.45 GB" "the dip is recorded with its measurement line"
    assert_says "$OUT/warndip.out" "it is not sustained" "the sustain half is named as the one that declined"
    assert_silent_about "$OUT/warndip.out" "it is uncorroborated" "the corroboration note is not printed, because it was corroborated"
}

test_the_boundary_reading_alone_satisfies_the_sustain_arm() {
    echo "test: available under the floor at the boundary is sustained, with the window healthy"
    local rc
    reset_state
    # Every sample is well clear of the floor, so only the boundary's own reading
    # can be what satisfied sustain. That is the first arm, and it needs no window.
    # Warn pressure at the boundary supplies the corroboration.
    printf '1 6.00 30.00 0.00\n1 6.10 29.00 0.00\n' > "$HOST_MEMORY_SAMPLES_FILE"
    HOST_PRESSURE_LEVEL_OVERRIDE=2 HOST_COMPRESSOR_GB_OVERRIDE=6.20 \
        HOST_AVAIL_GB_OVERRIDE=9.00 HOST_SWAP_USED_GB_OVERRIDE=0.00 \
        HOST_PHYSMEM_GB_OVERRIDE=48 \
        check_host_memory_at_boundary "nav chunk 11/34" >"$OUT/floornow.out" 2>&1
    rc=$?
    assert_eq "1" "$rc" "the boundary reading satisfies sustain and the run stops"
    assert_says "$OUT/floornow.out" "STOP: available memory 9.00 GB is under the 9.60 GB floor" "the stop names the floor it broke"
    assert_says "$OUT/floornow.out" "CORROBORATED SCARCITY" "the stop names itself as corroborated scarcity"
}

test_two_under_floor_samples_stop_even_after_the_boundary_recovered() {
    echo "test: two samples under the floor stop the run, though the boundary recovered"
    local rc
    reset_state
    # Sustained scarcity that ended just before the boundary, corroborated by two
    # warn samples in the same window. Both second arms, and the boundary itself
    # reads healthy on both dimensions, so only the window can be what caught it.
    printf '2 6.00 9.40 0.00\n2 6.10 9.20 0.00\n1 6.20 30.00 0.00\n' > "$HOST_MEMORY_SAMPLES_FILE"
    HOST_PRESSURE_LEVEL_OVERRIDE=1 HOST_COMPRESSOR_GB_OVERRIDE=6.30 \
        HOST_AVAIL_GB_OVERRIDE=20.00 HOST_SWAP_USED_GB_OVERRIDE=0.00 \
        HOST_PHYSMEM_GB_OVERRIDE=48 \
        check_host_memory_at_boundary "nav chunk 12/34" >"$OUT/twounder.out" 2>&1
    rc=$?
    assert_eq "1" "$rc" "two under-floor samples with two warn samples stop the run"
    assert_says "$OUT/twounder.out" "STOP: available memory 9.20 GB is under the 9.60 GB floor" "the stop quotes the window minimum"
    assert_says "$OUT/twounder.out" "the kernel at warn or worse in 2 of 3 samples" "the window arm of the corroboration is named"
    assert_says "$OUT/twounder.out" "CORROBORATED SCARCITY" "the stop names itself as corroborated scarcity"
}

# One warn sample is not corroboration, the same way one critical tick is not a
# stop and one dip is not sustain. Both halves are missing here, so both notes
# print and the reader sees which one declined.
test_a_single_warn_sample_does_not_corroborate() {
    echo "test: sustained sub-floor headroom with one warn sample runs on, and says why"
    local rc
    reset_state
    printf '2 6.00 9.40 0.00\n1 6.10 9.20 0.00\n1 6.20 30.00 0.00\n' > "$HOST_MEMORY_SAMPLES_FILE"
    HOST_PRESSURE_LEVEL_OVERRIDE=1 HOST_COMPRESSOR_GB_OVERRIDE=6.30 \
        HOST_AVAIL_GB_OVERRIDE=20.00 HOST_SWAP_USED_GB_OVERRIDE=0.00 \
        HOST_PHYSMEM_GB_OVERRIDE=48 \
        check_host_memory_at_boundary "nav chunk 13/34" >"$OUT/onewarn.out" 2>&1
    rc=$?
    assert_eq "0" "$rc" "one warn sample does not corroborate the floor"
    assert_silent_about "$OUT/onewarn.out" "STOP:" "no stop was announced"
    assert_says "$OUT/onewarn.out" "warn or worse in 1 of 3 samples" "the note counts the warn samples it saw"
}

# Only the stop DECISION changed. The deepest excursion still reaches the log, so
# a survived dip can be read at 06:30 rather than being silently swallowed.
test_a_survived_dip_still_prints_the_window_minimum() {
    echo "test: a dip the run survived is still visible on the boundary line"
    reset_state
    printf '1 6.00 30.00 0.00\n1 6.50 9.45 0.00\n1 6.20 29.00 0.00\n' > "$HOST_MEMORY_SAMPLES_FILE"
    HOST_PRESSURE_LEVEL_OVERRIDE=1 HOST_COMPRESSOR_GB_OVERRIDE=6.30 \
        HOST_AVAIL_GB_OVERRIDE=13.48 HOST_SWAP_USED_GB_OVERRIDE=0.00 \
        HOST_PHYSMEM_GB_OVERRIDE=48 \
        check_host_memory_at_boundary "nav chunk 4/34" >"$OUT/dipline.out" 2>&1
    assert_says "$OUT/dipline.out" "available 9.45 GB" "the boundary line carries the window minimum"
    assert_says "$OUT/dipline.out" "worst of 3 samples" "the scope wording still names the window"
    assert_says "$OUT/dipline.out" "note: available dipped to 9.45 GB, under the 9.60 GB floor, in 1 of 3 samples" "the note counts the dips it saw"
    assert_says "$OUT/dipline.out" "read 13.48 GB at the boundary" "the note states the boundary reading that let the run continue"
    # Both halves were missing here, so both reason lines print. A reader at 06:30
    # has to be able to tell which one declined, and this dip failed on both.
    assert_says "$OUT/dipline.out" "it is not sustained" "the sustain half is named"
    assert_says "$OUT/dipline.out" "it is uncorroborated" "the corroboration half is named too"
}

# A zero reading is a READING, not an absence. Swap is 0.00 on every sample this
# host produces, and a freshly booted host reads a 0.00 compressor, so a fold
# that used 0 as its "nothing seen" sentinel reported both as unreadable.
test_a_zero_reading_in_the_window_is_a_value_not_an_absence() {
    echo "test: an all-zero swap and compressor window reports 0.00, not unreadable"
    local got
    reset_state
    printf '1 0.00 30.00 0.00\n1 0.00 29.00 0.00\n' > "$HOST_MEMORY_SAMPLES_FILE"
    got="$(_host_mem_window_worst)"
    assert_eq "1 0.00 29.00 0.00 0 0 0 2" "$got" "zero compressor and zero swap survive the fold"
}

test_an_unreadable_dimension_in_the_window_never_erases_the_direct_read() {
    echo "test: a '-' dimension in the window leaves the boundary's own reading standing"
    local rc
    reset_state
    # Every sample failed to read available memory. The direct read did not, and
    # it is under the floor, so the floor must still fire once corroborated.
    printf '1 6.00 - 0.00\n1 6.10 - 0.00\n' > "$HOST_MEMORY_SAMPLES_FILE"
    HOST_PRESSURE_LEVEL_OVERRIDE=2 HOST_COMPRESSOR_GB_OVERRIDE=6.10 \
        HOST_AVAIL_GB_OVERRIDE=4.00 HOST_SWAP_USED_GB_OVERRIDE=0.00 \
        HOST_PHYSMEM_GB_OVERRIDE=48 \
        check_host_memory_at_boundary "nav chunk 8/34" >"$OUT/partial.out" 2>&1
    rc=$?
    assert_eq "1" "$rc" "the floor still fires on the direct reading"
    assert_says "$OUT/partial.out" "available memory 4.00 GB is under" "the direct available reading survived the fold"
}

test_no_samples_falls_back_to_a_single_reading() {
    echo "test: with no sampler the boundary behaves exactly as it did before"
    local rc
    reset_state
    HOST_PRESSURE_LEVEL_OVERRIDE=1 HOST_COMPRESSOR_GB_OVERRIDE=12.00 \
        HOST_AVAIL_GB_OVERRIDE=30.00 HOST_SWAP_USED_GB_OVERRIDE=0.00 \
        HOST_PHYSMEM_GB_OVERRIDE=48 \
        check_host_memory_at_boundary "nav chunk 2/34" >"$OUT/nosampler.out" 2>&1
    rc=$?
    assert_eq "0" "$rc" "a healthy host with no samples runs on"
    assert_says "$OUT/nosampler.out" "(single reading)" "the line says it judged one reading"
}

# ── Test 8: the sampler is run-scoped and signals only its own pid ───────────
test_the_sampler_starts_writes_and_is_reaped() {
    echo "test: the sampler runs, appends, and dies when stopped"
    local pid n
    reset_state
    LUCIDOS_E2E_MEM_POLL_SECS=1 HOST_PRESSURE_LEVEL_OVERRIDE=1 \
        HOST_COMPRESSOR_GB_OVERRIDE=5.00 HOST_AVAIL_GB_OVERRIDE=30.00 \
        HOST_SWAP_USED_GB_OVERRIDE=0.00 \
        start_host_memory_sampler >"$OUT/sampler.out" 2>&1
    pid="$HOST_MEMORY_SAMPLER_PID"
    if [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null; then
        pass "the sampler loop is running (pid=$pid)"
    else
        fail "the sampler did not start"
        return
    fi
    assert_eq "$pid" "$(cat "$HOST_MEMORY_SAMPLER_PIDFILE" 2>/dev/null)" "the pidfile records the loop's pid"
    sleep 2
    n="$(wc -l < "$HOST_MEMORY_SAMPLES_FILE" | tr -d ' ')"
    if [ "${n:-0}" -ge 1 ]; then
        pass "the loop appended $n sample(s)"
    else
        fail "the loop appended nothing"
    fi
    stop_host_memory_sampler
    if kill -0 "$pid" 2>/dev/null; then
        fail "the sampler survived stop_host_memory_sampler"
    else
        pass "the sampler is reaped"
    fi
    assert_eq "" "$HOST_MEMORY_SAMPLER_PID" "the in-memory handle is cleared"
    if [ -f "$HOST_MEMORY_SAMPLER_PIDFILE" ]; then
        fail "the pidfile survived the stop"
    else
        pass "the pidfile is removed"
    fi
    # The window is this run's, so it must not outlive it. A file left behind
    # would be folded into the next run's first boundary, judging it against a
    # host that no longer exists.
    if [ -f "$HOST_MEMORY_SAMPLES_FILE" ]; then
        fail "the samples file survived the stop"
    else
        pass "the samples file is removed"
    fi
    # Idempotent: a second stop with nothing running must be a silent no-op, which
    # is what makes it safe in an EXIT trap.
    stop_host_memory_sampler
    pass "a second stop is a no-op"
}

test_the_sampler_never_signals_a_pid_it_did_not_spawn() {
    echo "test: a junk pidfile is discarded rather than signalled"
    local killed="$SANDBOX/kill-attempts"
    reset_state
    : > "$killed"
    # A pidfile naming a pid this run never started. The only lethal call the stop
    # may make is against its OWN recorded pid, so with none recorded it must make
    # none at all (ADR 0025).
    echo "not-a-pid" > "$HOST_MEMORY_SAMPLER_PIDFILE"
    (
        kill() {
            echo "$*" >> "$killed"
            return 0
        }
        stop_host_memory_sampler
    ) >"$OUT/junkpid.out" 2>&1
    if [ -s "$killed" ]; then
        fail "a non-numeric pidfile produced a kill: $(cat "$killed")"
    else
        pass "a non-numeric pidfile signals nothing"
    fi
}

# ── Test 8b: the in-chunk stop ──────────────────────────────────────────
# A hung chunk never reaches a boundary, so the sampler itself watches for the
# freeze signature. These drive one tick at a time through the override seams.

# Run $1 ticks at the current overrides, carrying the streak, and print it.
run_ticks() {
    local n="$1" collapse="$2" ticks="$3" s=0 i
    for i in $(seq 1 "$n"); do
        s="$(_host_mem_sampler_tick "$HOST_MEMORY_SAMPLES_FILE" "$s" "$collapse" "$ticks")"
    done
    echo "streak=$s"
}

# $3 ticks at pressure $1 and available $2 GB, against a 2.40 GB collapse level.
ticks_at() {
    HOST_PRESSURE_LEVEL_OVERRIDE="$1" HOST_AVAIL_GB_OVERRIDE="$2" \
        HOST_COMPRESSOR_GB_OVERRIDE=17.94 HOST_SWAP_USED_GB_OVERRIDE=0.00 run_ticks "$3" 2.40 3
}

test_a_collapse_streak_trips_the_chunk() {
    echo "test: three collapse samples in a row trip the chunk, once"
    local stops="$SANDBOX/runner-stops"
    reset_state
    : > "$stops"
    (
        # shellcheck disable=SC2329 # a seam: invoked by the sourced guard, not from this file
        _host_mem_stop_runner() { echo stop >> "$stops"; }
        ticks_at 4 0.30 2
        [ -e "$HOST_MEMORY_TRIP_FILE" ] && echo "tripped-early"
        ticks_at 4 0.30 3
        ticks_at 4 0.30 2
    ) >"$OUT/trip.out" 2>&1
    assert_silent_about "$OUT/trip.out" "tripped-early" "two collapse samples do not trip"
    if [ -s "$HOST_MEMORY_TRIP_FILE" ]; then
        pass "the third collapse sample writes the trip"
    else
        fail "no trip after three collapse samples"
    fi
    assert_says "$HOST_MEMORY_TRIP_FILE" "0.30 GB" "the trip names the available reading"
    assert_says "$HOST_MEMORY_TRIP_FILE" "2.40 GB collapse level" "the trip names the collapse level"
    assert_says "$OUT/trip.out" "STOP inside a chunk" "the run log announces the stop"
    assert_eq "1" "$(wc -l < "$stops" | tr -d ' ')" "the runner is interrupted exactly once"
    assert_eq "7" "$(wc -l < "$HOST_MEMORY_SAMPLES_FILE" | tr -d ' ')" "every tick still appends its sample"
}

test_only_the_freeze_signature_trips_mid_chunk() {
    echo "test: critical alone, warn, and a deep dip without critical never trip"
    local case_ level avail
    for case_ in "4 11.00" "4 2.41" "2 0.30" "1 0.30"; do
        level="${case_%% *}"
        avail="${case_##* }"
        reset_state
        (
            # shellcheck disable=SC2329 # a seam: invoked by the sourced guard, not from this file
            _host_mem_stop_runner() { :; }
            ticks_at "$level" "$avail" 10
        ) >"$OUT/notrip.out" 2>&1
        if [ -e "$HOST_MEMORY_TRIP_FILE" ]; then
            fail "pressure $level with $avail GB available tripped the chunk"
        else
            pass "pressure $level with $avail GB available runs on for ten samples"
        fi
    done
    # A compressor past the runaway backstop is a boundary rule, never a trip.
    reset_state
    (
        # shellcheck disable=SC2329 # a seam: invoked by the sourced guard, not from this file
        _host_mem_stop_runner() { :; }
        HOST_PRESSURE_LEVEL_OVERRIDE=1 HOST_AVAIL_GB_OVERRIDE=30.00 \
            HOST_COMPRESSOR_GB_OVERRIDE=30.00 HOST_SWAP_USED_GB_OVERRIDE=0.00 run_ticks 10 2.40 3
    ) >"$OUT/bigcomp.out" 2>&1
    if [ -e "$HOST_MEMORY_TRIP_FILE" ]; then
        fail "a 30 GB compressor tripped the chunk"
    else
        pass "a 30 GB compressor runs on for ten samples"
    fi
    # At the level counts, the same "at or under" the boundary's collapse arm uses.
    reset_state
    (
        # shellcheck disable=SC2329 # a seam: invoked by the sourced guard, not from this file
        _host_mem_stop_runner() { :; }
        ticks_at 4 2.40 3
    ) >"$OUT/attrip.out" 2>&1
    if [ -e "$HOST_MEMORY_TRIP_FILE" ]; then
        pass "available exactly at the collapse level trips"
    else
        fail "available exactly at the collapse level did not trip"
    fi
}

test_a_broken_streak_starts_over() {
    echo "test: one healthy sample resets the streak"
    reset_state
    (
        # shellcheck disable=SC2329 # a seam: invoked by the sourced guard, not from this file
        _host_mem_stop_runner() { :; }
        s=0
        for reading in "4 0.30" "4 0.30" "4 11.00" "4 0.30" "4 0.30" "4 0.30"; do
            [ -e "$HOST_MEMORY_TRIP_FILE" ] && echo "tripped-on-a-broken-streak"
            s="$(HOST_PRESSURE_LEVEL_OVERRIDE="${reading%% *}" HOST_AVAIL_GB_OVERRIDE="${reading##* }" \
                HOST_COMPRESSOR_GB_OVERRIDE=17.00 HOST_SWAP_USED_GB_OVERRIDE=0.00 \
                _host_mem_sampler_tick "$HOST_MEMORY_SAMPLES_FILE" "$s" 2.40 3)"
        done
        echo "streak=$s"
    ) >"$OUT/broken.out" 2>&1
    assert_silent_about "$OUT/broken.out" "tripped-on-a-broken-streak" "two plus two collapse samples do not add up"
    assert_says "$OUT/broken.out" "streak=3" "the streak counts only the unbroken run"
    if [ -e "$HOST_MEMORY_TRIP_FILE" ]; then
        pass "the third sample of the new run trips"
    else
        fail "the new run of three did not trip"
    fi
}

test_the_trip_ticks_knob_falls_back() {
    echo "test: LUCIDOS_E2E_COLLAPSE_TICKS takes a positive integer or the default"
    assert_eq "3" "$(_host_mem_trip_ticks)" "the default is three samples"
    assert_eq "1" "$(LUCIDOS_E2E_COLLAPSE_TICKS=1 _host_mem_trip_ticks)" "an explicit 1 is honoured"
    assert_eq "3" "$(LUCIDOS_E2E_COLLAPSE_TICKS=0 _host_mem_trip_ticks)" "0 falls back rather than tripping on every sample"
    assert_eq "3" "$(LUCIDOS_E2E_COLLAPSE_TICKS=3s _host_mem_trip_ticks)" "a typo falls back"
}

# A synthetic process tree and a kill shim for the runner stop. The shim keeps
# a liveness file, so `kill -0` answers what the earlier signals did. It never
# reaches the real kill: every signal is recorded and nothing else happens.
TREE_KILLS="$SANDBOX/tree-kills"
TREE_ALIVE="$SANDBOX/tree-alive"
TREE_WAITS="$SANDBOX/tree-waits"
tree_reset() {
    : > "$TREE_KILLS"
    : > "$TREE_WAITS"
    printf '%s\n' 4242 4243 4244 6000 5000 900 > "$TREE_ALIVE"
}

# $1 = 1 when SIGINT ends a process, 0 when the tree ignores it. The rest is
# the command to run under the shims, _host_mem_stop_runner by default.
tree_stop_runner() {
    local int_kills="$1"
    shift
    (
        # shellcheck disable=SC2329 # a seam: invoked by the sourced guard, not from this file
        _host_mem_proc_tree() { printf '%s\n' "900 1" "4242 900" "4243 4242" "4244 4243" "6000 4244" "5000 1"; }
        # shellcheck disable=SC2329 # a seam: invoked by the sourced guard, not from this file
        _host_mem_trip_grace_wait() { echo wait >> "$TREE_WAITS"; }
        # shellcheck disable=SC2329 # shadows the builtin for the guard under test
        kill() {
            local sig="$1" pid="$2"
            case "$sig" in
                -0) grep -qx "$pid" "$TREE_ALIVE" ;;
                *)
                    echo "$sig $pid" >> "$TREE_KILLS"
                    if [ "$sig" = "-KILL" ] || [ "$int_kills" = 1 ]; then
                        grep -vx "$pid" "$TREE_ALIVE" > "$TREE_ALIVE.new"
                        mv "$TREE_ALIVE.new" "$TREE_ALIVE"
                    fi
                    return 0
                    ;;
            esac
        }
        if [ "$#" -gt 0 ]; then "$@"; else _host_mem_stop_runner; fi
    )
}

test_a_trip_interrupts_only_the_recorded_invocation() {
    echo "test: the stop signals the recorded pid and its descendants, nothing else"
    reset_state
    tree_reset
    echo 4242 > "$HOST_MEMORY_RUNNER_PIDFILE"
    tree_stop_runner 1 >"$OUT/tree.out" 2>&1
    local pid
    for pid in 4242 4243 4244 6000; do
        assert_says "$TREE_KILLS" "-INT $pid" "pid $pid in the runner's tree is interrupted"
    done
    assert_silent_about "$TREE_KILLS" " 5000" "an unrelated process is never signalled"
    assert_silent_about "$TREE_KILLS" " 900" "the runner's parent is never signalled"
    assert_silent_about "$TREE_KILLS" "-KILL" "a tree that exits on the interrupt is not killed"
}

test_a_runner_that_ignores_the_interrupt_is_killed() {
    echo "test: a tree still standing after the grace is killed"
    reset_state
    tree_reset
    echo 4242 > "$HOST_MEMORY_RUNNER_PIDFILE"
    LUCIDOS_E2E_TRIP_GRACE_SECS=3 tree_stop_runner 0 >"$OUT/treekill.out" 2>&1
    assert_eq "3" "$(wc -l < "$TREE_WAITS" | tr -d ' ')" "the grace knob sets how long it waits"
    local pid
    for pid in 4242 4243 4244 6000; do
        assert_says "$TREE_KILLS" "-KILL $pid" "pid $pid is killed after the grace"
    done
    assert_silent_about "$TREE_KILLS" " 5000" "an unrelated process is still never signalled"
}

test_a_trip_with_no_safe_runner_signals_nothing() {
    echo "test: no pidfile, a junk one, pid 1 or a protected pid means no signal"
    local content
    for content in "" "not-a-pid" "1" "31337"; do
        reset_state
        tree_reset
        [ -n "$content" ] && echo "$content" > "$HOST_MEMORY_RUNNER_PIDFILE"
        tree_stop_runner 1 >"$OUT/nosafe.out" 2>&1
        if [ -s "$TREE_KILLS" ]; then
            fail "runner pidfile '${content:-absent}' produced a signal: $(tr '\n' ' ' < "$TREE_KILLS")"
        else
            pass "runner pidfile '${content:-absent}' signals nothing"
        fi
    done
    reset_state
    tree_reset
    echo 4242 > "$HOST_MEMORY_RUNNER_PIDFILE"
    (
        # shellcheck disable=SC2329 # a seam: invoked by the sourced guard, not from this file
        is_protected_host_pid() { [ "$1" = 4242 ]; }
        tree_stop_runner 1
    ) >"$OUT/protected.out" 2>&1
    if [ -s "$TREE_KILLS" ]; then
        fail "a protected runner pid was signalled: $(tr '\n' ' ' < "$TREE_KILLS")"
    else
        pass "a protected runner pid is left alone"
    fi
}

test_an_empty_tree_feed_signals_only_the_runner() {
    echo "test: an empty process listing means no descendants, never the real host"
    reset_state
    tree_reset
    echo 4242 > "$HOST_MEMORY_RUNNER_PIDFILE"
    (
        # shellcheck disable=SC2329 # a seam: invoked by the sourced guard, not from this file
        _host_mem_proc_tree() { return 0; }
        # shellcheck disable=SC2329 # a seam: invoked by the sourced guard, not from this file
        _host_mem_trip_grace_wait() { :; }
        # shellcheck disable=SC2329 # shadows the builtin for the guard under test
        kill() {
            case "$1" in
                -0) grep -qx "$2" "$TREE_ALIVE" ;;
                *) echo "$1 $2" >> "$TREE_KILLS"; grep -vx "$2" "$TREE_ALIVE" > "$TREE_ALIVE.new"; mv "$TREE_ALIVE.new" "$TREE_ALIVE" ;;
            esac
        }
        _host_mem_stop_runner
    ) >"$OUT/emptytree.out" 2>&1
    assert_eq "-INT 4242" "$(tr -d '\r' < "$TREE_KILLS")" "only the recorded pid is interrupted"
}

test_teardown_interrupts_only_a_runner_this_run_recorded() {
    echo "test: a torn-down run interrupts its own runner, never one a stale file names"
    reset_state
    tree_reset
    echo 4242 > "$HOST_MEMORY_RUNNER_PIDFILE"
    HOST_MEMORY_RUNNER_PID="" tree_stop_runner 1 stop_host_memory_sampler >"$OUT/stale.out" 2>&1
    if [ -s "$TREE_KILLS" ]; then
        fail "a runner named only by a stale file was signalled: $(tr '\n' ' ' < "$TREE_KILLS")"
    else
        pass "a runner named only by a stale file is left alone"
    fi
    reset_state
    tree_reset
    echo 4242 > "$HOST_MEMORY_RUNNER_PIDFILE"
    HOST_MEMORY_RUNNER_PID=4242 tree_stop_runner 1 stop_host_memory_sampler >"$OUT/torn.out" 2>&1
    assert_says "$TREE_KILLS" "-INT 4242" "the runner this run recorded is interrupted"
    assert_says "$TREE_KILLS" "-INT 6000" "and so is its tree"
    assert_says "$OUT/torn.out" "interrupting the Playwright runner this run left running" "the teardown says so"
}

test_a_trip_while_the_runner_started_still_reaches_it() {
    echo "test: interrupt_host_memory_runner_if_tripped acts only on a recorded trip"
    reset_state
    tree_reset
    HOST_MEMORY_RUNNER_PID=4242 tree_stop_runner 1 interrupt_host_memory_runner_if_tripped >"$OUT/notrip2.out" 2>&1
    if [ -s "$TREE_KILLS" ]; then fail "no trip, yet the runner was signalled"; else pass "no trip means no signal"; fi
    reset_state
    tree_reset
    echo "Between two boundaries the kernel reported CRITICAL memory pressure." > "$HOST_MEMORY_TRIP_FILE"
    HOST_MEMORY_RUNNER_PID=4242 tree_stop_runner 1 interrupt_host_memory_runner_if_tripped >"$OUT/latetrip.out" 2>&1
    assert_says "$TREE_KILLS" "-INT 4242" "a trip recorded while the runner started interrupts it"
}

record_then_stop() {
    record_host_memory_runner 4242
    stop_host_memory_sampler
}

test_the_runner_record_and_the_trip_are_run_scoped() {
    echo "test: start and stop clear a stale trip and runner record"
    reset_state
    echo "stale detail" > "$HOST_MEMORY_TRIP_FILE"
    echo 4242 > "$HOST_MEMORY_RUNNER_PIDFILE"
    HOST_PRESSURE_LEVEL_OVERRIDE=1 HOST_COMPRESSOR_GB_OVERRIDE=5.00 HOST_AVAIL_GB_OVERRIDE=30.00 \
        HOST_SWAP_USED_GB_OVERRIDE=0.00 start_host_memory_sampler >"$OUT/scoped.out" 2>&1
    if [ -e "$HOST_MEMORY_TRIP_FILE" ] || [ -e "$HOST_MEMORY_RUNNER_PIDFILE" ]; then
        fail "a stale trip or runner record survived the sampler start"
    else
        pass "the start clears a stale trip and runner record"
    fi
    # Recording signals nothing, and the clear comes before any stop, so no real
    # pid is ever handed to the teardown's interrupt.
    record_host_memory_runner 4242
    assert_eq "4242" "$(cat "$HOST_MEMORY_RUNNER_PIDFILE" 2>/dev/null)" "the runner pid is recorded"
    assert_eq "4242" "$HOST_MEMORY_RUNNER_PID" "and held in memory"
    clear_host_memory_runner
    if [ -e "$HOST_MEMORY_RUNNER_PIDFILE" ] || [ -n "$HOST_MEMORY_RUNNER_PID" ]; then
        fail "the runner record survived its clear"
    else
        pass "the runner record is cleared"
    fi
    stop_host_memory_sampler >/dev/null 2>&1
    # The teardown with a runner still recorded runs under the tree shims, so its
    # interrupt reaches only the synthetic tree.
    tree_reset
    echo "detail" > "$HOST_MEMORY_TRIP_FILE"
    tree_stop_runner 1 record_then_stop >"$OUT/scopedstop.out" 2>&1
    if [ -e "$HOST_MEMORY_TRIP_FILE" ] || [ -e "$HOST_MEMORY_RUNNER_PIDFILE" ]; then
        fail "the trip or runner record survived the sampler stop"
    else
        pass "the stop removes the trip and runner record"
    fi
}

test_a_trip_is_read_back_as_a_memory_stop() {
    echo "test: host_memory_stopped_mid_chunk reads the trip and its detail"
    reset_state
    if host_memory_stopped_mid_chunk; then
        fail "no trip file still reads as a stop"
    else
        pass "no trip file reads as no stop"
    fi
    echo "Inside a running chunk the kernel reported CRITICAL memory pressure." > "$HOST_MEMORY_TRIP_FILE"
    if host_memory_stopped_mid_chunk; then
        pass "a trip file reads as a stop"
    else
        fail "a trip file did not read as a stop"
    fi
    assert_eq "Inside a running chunk the kernel reported CRITICAL memory pressure." \
        "$MEMORY_STOP_DETAIL" "the trip's detail becomes the stop detail"
}

test_a_boundary_after_a_trip_stops_on_it() {
    echo "test: a trip recorded between invocations stops the next boundary"
    local rc=0
    reset_state
    echo "Between two boundaries the kernel reported CRITICAL memory pressure." > "$HOST_MEMORY_TRIP_FILE"
    HOST_COMPRESSOR_GB_OVERRIDE=5.00 HOST_AVAIL_GB_OVERRIDE=30.00 HOST_SWAP_USED_GB_OVERRIDE=0.00 \
        HOST_PHYSMEM_GB_OVERRIDE=48 check_host_memory_at_boundary "the boundary after project chromium" \
        >"$OUT/tripboundary.out" 2>&1 || rc=$?
    assert_eq "1" "$rc" "a healthy reading does not clear a recorded trip"
    assert_says "$OUT/tripboundary.out" "already recorded the freeze signature" "the boundary says why it stopped"
    assert_eq "Between two boundaries the kernel reported CRITICAL memory pressure." \
        "$MEMORY_STOP_DETAIL" "the trip's detail becomes the stop detail"
}

test_the_reading_that_stopped_an_earlier_nightly_now_runs_on() {
    echo "test: 14.98 GB with no exported ceiling runs on"
    local rc
    reset_state
    HOST_COMPRESSOR_GB_OVERRIDE=14.98 HOST_AVAIL_GB_OVERRIDE=27 \
        HOST_SWAP_USED_GB_OVERRIDE=0.25 HOST_PHYSMEM_GB_OVERRIDE=48 \
        check_host_memory_at_boundary "mobile-webkit phase 1/2 (CC)" >"$OUT/nightly.out" 2>&1
    rc=$?
    assert_eq "0" "$rc" "the boundary that ended an earlier nightly no longer stops"
    assert_silent_about "$OUT/nightly.out" "STOP" "no stop was announced"
}

test_explicit_absolute_ceiling_overrides_the_share() {
    echo "test: LUCIDOS_E2E_COMPRESSOR_MAX_GB overrides the RAM share"
    local rc
    reset_state
    HOST_COMPRESSOR_GB_OVERRIDE=10.00 HOST_AVAIL_GB_OVERRIDE=30 \
        HOST_SWAP_USED_GB_OVERRIDE=0.00 HOST_PHYSMEM_GB_OVERRIDE=48 LUCIDOS_E2E_COMPRESSOR_MAX_GB=9 \
        check_host_memory_at_boundary "nav chunk 2/32" >"$OUT/abs.out" 2>&1
    rc=$?
    assert_eq "1" "$rc" "an explicit lower ceiling still stops the run"
    assert_says "$OUT/abs.out" "over the 9 GB backstop" "the explicit value is the one applied"

    # An operator who names a number above the share means that number.
    local got
    got="$(HOST_PHYSMEM_GB_OVERRIDE=48 LUCIDOS_E2E_COMPRESSOR_MAX_GB=30 \
        _host_mem_compressor_ceiling_gb)"
    assert_eq "30" "$got" "an explicit ceiling above the share is honored"
}

test_compressor_percent_knob_sets_the_runaway() {
    echo "test: LUCIDOS_E2E_COMPRESSOR_MAX_PCT moves the runaway backstop"
    local rc got
    reset_state
    # MAX_PCT=25 lowers the runaway to 12 GB, under the default 24. A 14 GB
    # compressor is over the lowered one, so the runaway is what stops it.
    HOST_COMPRESSOR_GB_OVERRIDE=14.00 HOST_AVAIL_GB_OVERRIDE=20.00 \
        HOST_SWAP_USED_GB_OVERRIDE=0.00 HOST_PHYSMEM_GB_OVERRIDE=48 LUCIDOS_E2E_COMPRESSOR_MAX_PCT=25 \
        check_host_memory_at_boundary "nav chunk 3/32" >"$OUT/pct.out" 2>&1
    rc=$?
    assert_eq "1" "$rc" "25% of 48 GB stops a 14 GB compressor on the runaway"
    assert_says "$OUT/pct.out" "over the 12.00 GB backstop" "the runaway is a quarter of RAM"

    # The runaway share has no flat cap, so a high percentage is honored.
    got="$(HOST_PHYSMEM_GB_OVERRIDE=48 LUCIDOS_E2E_COMPRESSOR_MAX_PCT=75 \
        _host_mem_compressor_ceiling_gb)"
    assert_eq "36.00" "$got" "75% of 48 GB is 36.00, not clamped"
}

# ── Test 5: a garbage knob must never stop the suite ─────────────────────────
test_garbage_knobs_fall_back_to_defaults() {
    echo "test: unusable overrides fall back to the defaults rather than stopping"
    local rc
    reset_state
    HOST_COMPRESSOR_GB_OVERRIDE=13.00 HOST_AVAIL_GB_OVERRIDE=30 \
        HOST_SWAP_USED_GB_OVERRIDE=0.00 HOST_PHYSMEM_GB_OVERRIDE=48 \
        LUCIDOS_E2E_COMPRESSOR_MAX_GB=lots LUCIDOS_E2E_COMPRESSOR_MAX_PCT=-4 \
        LUCIDOS_E2E_FREE_FLOOR_MIN_GB=nope LUCIDOS_E2E_FREE_FLOOR_PCT=bad \
        LUCIDOS_E2E_SWAP_MAX_GB=banana \
        check_host_memory_at_boundary "nav chunk 1/32" >"$OUT/junk.out" 2>&1
    rc=$?
    assert_eq "0" "$rc" "garbage knobs do not stop a healthy host"
    assert_says "$OUT/junk.out" "compressor 13.00 GB" "the boundary still reported the reading"
}

# A retired knob is worse than an unknown one: a caller who still sets it thinks
# the run is capped when nothing reads the value. The start report names it.
test_a_retired_cap_knob_is_named_rather_than_ignored() {
    echo "test: setting the retired compressor cap knob is called out at start"
    reset_state
    HOST_COMPRESSOR_GB_OVERRIDE=4.00 HOST_AVAIL_GB_OVERRIDE=30.00 \
        HOST_SWAP_USED_GB_OVERRIDE=0.00 HOST_PHYSMEM_GB_OVERRIDE=48 \
        LUCIDOS_E2E_COMPRESSOR_CAP_GB=16 \
        report_host_memory_start >"$OUT/retired.out" 2>&1
    assert_says "$OUT/retired.out" "no longer read" "the retired knob is named"

    reset_state
    HOST_COMPRESSOR_GB_OVERRIDE=4.00 HOST_AVAIL_GB_OVERRIDE=30.00 \
        HOST_SWAP_USED_GB_OVERRIDE=0.00 HOST_PHYSMEM_GB_OVERRIDE=48 \
        report_host_memory_start >"$OUT/notretired.out" 2>&1
    assert_silent_about "$OUT/notretired.out" "no longer read" "a run that does not set it hears nothing"
}

test_garbage_free_floor_knobs_fall_back() {
    echo "test: unusable free-floor knobs fall back, and 9 GB still stops on 48 GB"
    local rc
    reset_state
    # Garbage knobs must resolve to the 9.6 GB default floor, so 9 GB available
    # stops once warn pressure corroborates it.
    HOST_PRESSURE_LEVEL_OVERRIDE=2 HOST_COMPRESSOR_GB_OVERRIDE=6.00 HOST_AVAIL_GB_OVERRIDE=9.00 \
        HOST_SWAP_USED_GB_OVERRIDE=0.00 HOST_PHYSMEM_GB_OVERRIDE=48 \
        LUCIDOS_E2E_FREE_FLOOR_MIN_GB=lots LUCIDOS_E2E_FREE_FLOOR_PCT=-9 \
        check_host_memory_at_boundary "nav chunk 1/32" >"$OUT/floorjunk.out" 2>&1
    rc=$?
    assert_eq "1" "$rc" "garbage floor knobs fall back to the default that stops at 9 GB"
    assert_says "$OUT/floorjunk.out" "under the 9.60 GB floor" "the default floor applied"
}

test_garbage_swap_knob_does_not_stop_a_host_with_some_swap() {
    echo "test: an unusable swap knob falls back to 1 GB, not to zero"
    local rc
    reset_state
    HOST_COMPRESSOR_GB_OVERRIDE=5.00 HOST_AVAIL_GB_OVERRIDE=30 \
        HOST_SWAP_USED_GB_OVERRIDE=0.25 HOST_PHYSMEM_GB_OVERRIDE=48 LUCIDOS_E2E_SWAP_MAX_GB=banana \
        check_host_memory_at_boundary "nav chunk 1/32" >"$OUT/swapjunk.out" 2>&1
    rc=$?
    assert_eq "0" "$rc" "0.25 GB of swap is under the default 1 GB ceiling"
    assert_silent_about "$OUT/swapjunk.out" "STOP" "the boundary did not stop the run"
}

test_negative_percent_cannot_stop_everything() {
    echo "test: a zero-percent backstop falls back rather than stopping at once"
    local rc
    reset_state
    HOST_COMPRESSOR_GB_OVERRIDE=1.00 HOST_AVAIL_GB_OVERRIDE=30 \
        HOST_SWAP_USED_GB_OVERRIDE=0.00 HOST_PHYSMEM_GB_OVERRIDE=48 LUCIDOS_E2E_COMPRESSOR_MAX_PCT=0 \
        check_host_memory_at_boundary "nav chunk 1/32" >"$OUT/zero.out" 2>&1
    rc=$?
    assert_eq "0" "$rc" "a 0% backstop is rejected, so 1 GB passes"
}

# ── Test 6: fail open when the host cannot be measured ───────────────────────
test_unreadable_host_fails_open() {
    echo "test: an unmeasurable host skips the check instead of stopping"
    local rc
    reset_state
    # Shadow both host commands so every reader comes back empty, and drop the
    # file-wide pressure pin with them: a host nothing can be read from includes
    # the kernel's pressure oid. A guard that cannot measure must never end a run.
    # A subshell so the unset cannot leak into the tests after this one.
    (
        unset HOST_PRESSURE_LEVEL_OVERRIDE
        # shellcheck disable=SC2329 # a seam: invoked by the sourced guard, not from this file
        vm_stat() { return 1; }
        # shellcheck disable=SC2329 # a seam: invoked by the sourced guard, not from this file
        sysctl() { return 1; }
        check_host_memory_at_boundary "nav chunk 7/32"
    ) >"$OUT/blind.out" 2>&1
    rc=$?
    assert_eq "0" "$rc" "an unreadable host returns 0"
    assert_says "$OUT/blind.out" "host memory unreadable, check skipped" "it says why it skipped"
}

test_unreadable_ram_drops_the_compressor_backstop_but_keeps_swap() {
    echo "test: with RAM unreadable the compressor backstop lapses and swap still stops"
    local rc
    reset_state
    # No physical-memory reading and no explicit ceiling, so the compressor share
    # cannot be computed. A huge compressor must then pass, while swap still bites.
    # Available is injected high so the minimum floor does not fire.
    # shellcheck disable=SC2329 # a seam: invoked by the sourced guard, not from this file
    sysctl() { return 1; }
    HOST_COMPRESSOR_GB_OVERRIDE=99.00 HOST_AVAIL_GB_OVERRIDE=30 HOST_SWAP_USED_GB_OVERRIDE=0.00 \
        check_host_memory_at_boundary "nav chunk 8/32" >"$OUT/noram.out" 2>&1
    rc=$?
    assert_eq "0" "$rc" "no computable compressor backstop means the compressor cannot stop the run"

    reset_state
    HOST_COMPRESSOR_GB_OVERRIDE=99.00 HOST_AVAIL_GB_OVERRIDE=30 HOST_SWAP_USED_GB_OVERRIDE=4.00 \
        check_host_memory_at_boundary "nav chunk 8/32" >"$OUT/noram2.out" 2>&1
    rc=$?
    unset -f sysctl
    assert_eq "1" "$rc" "swap still stops the run with no compressor backstop available"
}

# ── Test 7: parsing, the places a unit mistake is silent ────────────────────
test_swap_units_are_read_off_the_value() {
    echo "test: vm.swapusage is parsed in M, G and K"
    local got
    # shellcheck disable=SC2329 # a seam: invoked by the sourced guard, not from this file
    sysctl() { echo "total = 0.00M  used = 0.00M  free = 0.00M  (encrypted)"; }
    got="$(_host_mem_read_swap_used_gb)"
    assert_eq "0.00" "$got" "0.00M reads as 0.00 GB"

    # shellcheck disable=SC2329 # a seam: invoked by the sourced guard, not from this file
    sysctl() { echo "total = 16384.00M  used = 14336.00M  free = 2048.00M  (encrypted)"; }
    got="$(_host_mem_read_swap_used_gb)"
    assert_eq "14.00" "$got" "14336.00M reads as 14.00 GB"

    # shellcheck disable=SC2329 # a seam: invoked by the sourced guard, not from this file
    sysctl() { echo "total = 20.00G  used = 2.50G  free = 17.50G  (encrypted)"; }
    got="$(_host_mem_read_swap_used_gb)"
    assert_eq "2.50" "$got" "2.50G reads as 2.50 GB, not 0.00"

    # shellcheck disable=SC2329 # a seam: invoked by the sourced guard, not from this file
    sysctl() { echo "total = 512.00K  used = 512.00K  free = 0.00K  (encrypted)"; }
    got="$(_host_mem_read_swap_used_gb)"
    assert_eq "0.00" "$got" "512.00K reads as effectively no swap"
    unset -f sysctl
}

test_compressor_uses_the_reported_page_size() {
    echo "test: vm_stat's own page size is used for the compressor, not a hardcoded 4 KB"
    local got
    # Apple silicon reports 16384. 65536 pages at 16 KB is exactly 1 GB; read with
    # a 4 KB constant it would come back as 0.25 and never trip anything.
    # shellcheck disable=SC2329 # a seam: invoked by the sourced guard, not from this file
    vm_stat() {
        echo "Mach Virtual Memory Statistics: (page size of 16384 bytes)"
        echo "Pages occupied by compressor:              65536."
    }
    got="$(_host_mem_read_compressor_gb)"
    assert_eq "1.00" "$got" "65536 pages of 16 KB reads as 1.00 GB"
    unset -f vm_stat
}

test_available_sums_free_speculative_purgeable_filebacked() {
    echo "test: available sums free + speculative + purgeable + file-backed, matching the gate"
    local got
    # 65536 pages of 16 KB is 1 GB each, so the four buckets add to 4.00 GB. This is
    # the pre-flight gate's definition; inactive is deliberately NOT summed. A reader
    # that summed inactive instead, missed a bucket, or used 4 KB would come back wrong.
    # shellcheck disable=SC2329 # a seam: invoked by the sourced guard, not from this file
    vm_stat() {
        echo "Mach Virtual Memory Statistics: (page size of 16384 bytes)"
        echo "Pages free:                                   65536."
        echo "Pages inactive:                               99999."
        echo "Pages speculative:                            65536."
        echo "Pages purgeable:                              65536."
        echo "File-backed pages:                            65536."
    }
    got="$(_host_mem_read_available_gb)"
    assert_eq "4.00" "$got" "four buckets of 1 GB read as 4.00 GB, inactive ignored"
    unset -f vm_stat
}

# ── Test 8: the reports ─────────────────────────────────────────────────────
test_start_records_the_baseline_and_states_the_thresholds() {
    echo "test: the start line records the baseline and names all four thresholds"
    reset_state
    HOST_COMPRESSOR_GB_OVERRIDE=4.39 HOST_AVAIL_GB_OVERRIDE=30.00 \
        HOST_SWAP_USED_GB_OVERRIDE=0.00 HOST_PHYSMEM_GB_OVERRIDE=48 \
        report_host_memory_start >"$OUT/start.out" 2>&1
    assert_eq "4.39" "$HOST_MEMORY_BASELINE_GB" "the baseline was recorded"
    assert_says "$OUT/start.out" "browser phase start: pressure normal, compressor 4.39 GB, available 30.00 GB, swap 0.00 GB" "the start line states all four readings"
    assert_says "$OUT/start.out" \
        "stops at: kernel pressure critical (the freeze signature), swap over 1 GB (distress), available under 9.60 GB WITH a corroborating signal (corroborated scarcity), or compressor over 24.00 GB (runaway backstop)." \
        "the four thresholds are stated up front, and labelled"
    assert_says "$OUT/start.out" "The corroborating signals are the kernel at warn or worse, or any swap in use." "the start line names what corroborates the floor"
    assert_says "$OUT/start.out" "Available under the floor on its own is recorded and never stops the run." "the start line says a bare available reading is not a stop"
    assert_says "$OUT/start.out" "no survivability cap" "the start line says the compressor cap is gone"
}

# ── Test 8b: the stops must not read alike ──────────────────────────────────
# A reader at 06:30 decides from these lines whether the Mac was in trouble.
test_the_swap_stop_calls_itself_measured_distress() {
    echo "test: the swap stop says the host is in trouble, in plain words"
    reset_state
    HOST_COMPRESSOR_GB_OVERRIDE=9.00 HOST_AVAIL_GB_OVERRIDE=30 \
        HOST_SWAP_USED_GB_OVERRIDE=2.40 HOST_PHYSMEM_GB_OVERRIDE=48 \
        check_host_memory_at_boundary "nav chunk 12/33" >"$OUT/distress.out" 2>&1
    assert_says "$OUT/distress.out" "MEASURED DISTRESS" "the swap stop names itself as distress"
    assert_says "$OUT/distress.out" "the host is in" "it says the host is in trouble"
    assert_silent_about "$OUT/distress.out" "BACKSTOP" "it does not also claim to be the backstop"
    assert_silent_about "$OUT/distress.out" "SURVIVABILITY" "it does not also claim to be the cap"
    assert_silent_about "$OUT/distress.out" "CORROBORATED SCARCITY" "it does not also claim scarcity"
    case "$MEMORY_STOP_DETAIL" in
        *"measured distress"*) pass "the final-verdict detail carries the classification" ;;
        *) fail "detail did not classify the stop: '$MEMORY_STOP_DETAIL'" ;;
    esac
}

test_the_compressor_stop_says_the_host_was_never_in_trouble() {
    echo "test: the runaway stop quotes the swap reading that proves it"
    reset_state
    HOST_COMPRESSOR_GB_OVERRIDE=25.00 HOST_AVAIL_GB_OVERRIDE=20.00 \
        HOST_SWAP_USED_GB_OVERRIDE=0.25 HOST_PHYSMEM_GB_OVERRIDE=48 \
        check_host_memory_at_boundary "nav chunk 30/33" >"$OUT/backstop.out" 2>&1
    assert_says "$OUT/backstop.out" "RUNAWAY BACKSTOP, NOT distress" "the stop names itself as the backstop"
    assert_says "$OUT/backstop.out" "Swap is still clear at" "it states the swap reading, not just the word clear"
    assert_says "$OUT/backstop.out" "0.25 GB, under its 1 GB limit" "the reading and the limit it cleared are both named"
    assert_silent_about "$OUT/backstop.out" "MEASURED DISTRESS" "it does not also claim distress"
    case "$MEMORY_STOP_DETAIL" in
        *"not in distress"*) pass "the final-verdict detail carries the classification" ;;
        *) fail "detail did not classify the stop: '$MEMORY_STOP_DETAIL'" ;;
    esac
}

# "Not distress" is a claim about SWAP, so it cannot be made when swap was not
# read. The compressor and available come from vm_stat and swap from sysctl, so
# swap can fail alone. Pressure also comes from sysctl, but the file-wide
# HOST_PRESSURE_LEVEL_OVERRIDE keeps it readable, so swap is the only blind
# dimension here and the runaway backstop is the stop that trips.
test_the_backstop_stop_does_not_clear_a_host_it_could_not_read() {
    echo "test: with swap unreadable the backstop stop refuses to say the host was fine"
    local rc
    reset_state
    # shellcheck disable=SC2329 # a seam: invoked by the sourced guard, not from this file
    sysctl() { return 1; }
    HOST_COMPRESSOR_GB_OVERRIDE=25.00 HOST_AVAIL_GB_OVERRIDE=20.00 HOST_PHYSMEM_GB_OVERRIDE=48 \
        check_host_memory_at_boundary "nav chunk 18/33" >"$OUT/blindswap.out" 2>&1
    rc=$?
    unset -f sysctl
    assert_eq "1" "$rc" "the backstop still stops the run"
    assert_says "$OUT/blindswap.out" "swap was UNREADABLE" "it says the swap reading is missing"
    assert_silent_about "$OUT/blindswap.out" "Swap is still clear" "it does not claim swap was clear"
    assert_silent_about "$OUT/blindswap.out" "never in" "it does not claim the host was never in trouble"
    case "$MEMORY_STOP_DETAIL" in
        *"could not be ruled out"*) pass "the final-verdict detail withholds the all-clear" ;;
        *) fail "detail claimed more than it knew: '$MEMORY_STOP_DETAIL'" ;;
    esac
}

test_stop_report_is_silent_on_a_run_that_finished() {
    echo "test: the final report says nothing when no stop happened"
    reset_state
    report_memory_stop >"$OUT/quiet.out" 2>&1
    if [ -s "$OUT/quiet.out" ]; then
        fail "the report spoke on a run that was never stopped"
        sed 's/^/      | /' "$OUT/quiet.out"
    else
        pass "the report stayed silent"
    fi
}

test_stop_report_states_what_this_run_itself_cost() {
    echo "test: the final report subtracts the baseline instead of asking the reader to"
    reset_state
    HOST_COMPRESSOR_GB_OVERRIDE=4.39 HOST_AVAIL_GB_OVERRIDE=30 \
        HOST_SWAP_USED_GB_OVERRIDE=0.00 HOST_PHYSMEM_GB_OVERRIDE=48 report_host_memory_start >/dev/null 2>&1
    HOST_COMPRESSOR_GB_OVERRIDE=11.50 HOST_AVAIL_GB_OVERRIDE=30 \
        HOST_SWAP_USED_GB_OVERRIDE=2.00 HOST_PHYSMEM_GB_OVERRIDE=48 \
        check_host_memory_at_boundary "nav chunk 20/32" >/dev/null 2>&1
    MEMORY_STOPPED="project mobile-webkit"
    report_memory_stop >"$OUT/stop.out" 2>&1

    assert_says "$OUT/stop.out" "STOPPED ON HOST MEMORY during project mobile-webkit." "the report names where it stopped"
    assert_says "$OUT/stop.out" "grew the compressor by 7.11 GB" "the report states this run's own cost"
    assert_says "$OUT/stop.out" "from a 4.39 GB baseline" "the report states the baseline it grew from"
    assert_says "$OUT/stop.out" "Exit 71 marks a memory stop, never a failing test." "the report says 71 is not a red project"
}

test_stop_report_does_not_advise_raising_the_ceiling() {
    echo "test: the advice is to free memory, never to raise a threshold"
    reset_state
    MEMORY_STOPPED="project mobile-webkit"
    MEMORY_STOP_DETAIL="whatever"
    report_memory_stop >"$OUT/advice.out" 2>&1
    assert_silent_about "$OUT/advice.out" "raises the ceiling" "no advice to raise the ceiling"
    assert_says "$OUT/advice.out" "Free memory on the host and rerun." "the advice is to free memory"
}

test_stop_exit_code_is_the_os_error_code() {
    echo "test: the memory stop carries 71, distinct from a Playwright verdict"
    assert_eq "71" "$HOST_MEMORY_STOP_EXIT" "HOST_MEMORY_STOP_EXIT is 71 (EX_OSERR)"
}

# ── Test 9: per-process attribution ─────────────────────────────────────────
# A synthetic host: the Docker VM, the e2e engine, one other engine, a WebKit
# content process, an e2e coding agent, a Playwright node, and (deliberately) the
# USER's own claude session. Every seam is injected, so the block is exercised
# without a real ps / footprint / lsof, exactly as the readings above are.
install_synthetic_attr_seams() {
    # pid<TAB>full-command, one per owned process. The full path is what the
    # classifier keys on.
    # shellcheck disable=SC2329 # a seam: invoked by the sourced guard, not from this file
    _host_mem_attr_proc_list() {
        printf '%s\t%s\n' \
            95531 "/System/Library/Frameworks/Virtualization.framework/XPCServices/com.apple.Virtualization.VirtualMachine" \
            33660 "/repo/.launch/release/e2e-test-hooks/lucidos-engine" \
            76951 "/Applications/Lucidos.app/Contents/Resources/lucidos-engine" \
            27539 "/ms-playwright/webkit-1/com.apple.WebKit.WebContent" \
            47654 "/Users/me/.local/bin/claude" \
            10546 "/opt/homebrew/bin/node" \
            48000 "/Users/me/.local/bin/claude"
    }
    # footprint -f bytes output. Only the header lines carry a per-process value;
    # the "Shared with" and "Summary" lines must be ignored by the parser.
    # shellcheck disable=SC2329 # a seam: invoked by the sourced guard, not from this file
    _host_mem_attr_footprint() {
        cat <<'FP'
======================================================================
com.apple.Virtualization.VirtualMachine [95531]: 64-bit    Footprint: 3221225472 B (16384 bytes per page)
======================================================================
Shared with lucidos-engine [33660], claude [47654]:
lucidos-engine [33660]: 64-bit    Footprint: 1073741824 B (16384 bytes per page)
lucidos-engine [76951]: 64-bit    Footprint: 536870912 B (16384 bytes per page)
com.apple.WebKit.WebContent [27539]: 64-bit    Footprint: 268435456 B (16384 bytes per page)
claude [47654]: 64-bit    Footprint: 209715200 B (16384 bytes per page)
node [10546]: 64-bit    Footprint: 104857600 B (16384 bytes per page)
Summary Footprint: 5414846464 B
FP
    }
    # 47654 is the e2e agent (cwd under the workspace); 10546 and 48000 sit
    # elsewhere, so the node is host demand (node-host) and the user's own claude
    # drops.
    # shellcheck disable=SC2329 # a seam: invoked by the sourced guard, not from this file
    _host_mem_attr_cwd() {
        case "$1" in
            47654) printf '%s' "$E2E_WORKSPACE/.lucidos/worktrees/wt-a" ;;
            *) printf '%s' "/Users/me/workspaces/dev/.lucidos/worktrees/other" ;;
        esac
    }
}

# Point E2E_WORKSPACE at a sandbox and write the engine pidfile the block reads.
setup_attr_workspace() {
    local pid="$1"
    export E2E_WORKSPACE="$SANDBOX/e2e-test"
    mkdir -p "$E2E_WORKSPACE/.lucidos"
    printf '%s' "$pid" >"$E2E_WORKSPACE/.lucidos/engine.pid"
}

test_attribution_block_is_emitted() {
    echo "test: the boundary emits a tagged per-process attribution block"
    local rc
    reset_state
    setup_attr_workspace 33660
    install_synthetic_attr_seams
    # A healthy host, so nothing stops: this proves the block runs on a run that
    # continues, not only on one that is about to stop.
    HOST_COMPRESSOR_GB_OVERRIDE=12.00 HOST_AVAIL_GB_OVERRIDE=30.00 \
        HOST_SWAP_USED_GB_OVERRIDE=0.00 HOST_PHYSMEM_GB_OVERRIDE=48 \
        check_host_memory_at_boundary "nav chunk 5/10" >"$OUT/attr.out" 2>&1
    rc=$?
    assert_eq "0" "$rc" "attribution does not stop a healthy host"
    assert_silent_about "$OUT/attr.out" "STOP" "no stop was announced"

    # Ranked top N, each kind distinguishable, footprint in MB.
    assert_says "$OUT/attr.out" "[e2e-mem-attr] after nav chunk 5/10: #1 com.apple.Virtualization.VirtualMachine pid=95531 3072 MB (docker-vm)" "the Docker VM ranks first and is labelled"
    assert_says "$OUT/attr.out" "#2 lucidos-engine pid=33660 1024 MB (engine)  <-- e2e engine" "the e2e engine is ranked and tagged"
    assert_says "$OUT/attr.out" "lucidos-engine pid=76951 512 MB (engine)" "the other engine is distinguishable"
    assert_says "$OUT/attr.out" "com.apple.WebKit.WebContent pid=27539 256 MB (browser)" "the WebKit content process is distinguishable"
    assert_says "$OUT/attr.out" "claude pid=47654 200 MB (agent)" "the coding agent is distinguishable"
    assert_says "$OUT/attr.out" "node pid=10546 100 MB (node-host)" "a node with no cwd under the run is host demand, not a run node"

    # The e2e engine is reported explicitly, on its own line.
    assert_says "$OUT/attr.out" "[e2e-mem-attr] after nav chunk 5/10: e2e engine pid=33660 1024 MB (primary suspect)" "the e2e engine gets its own explicit line"

    # The coding-agent aggregate counts only the e2e agent, never the user's own.
    assert_says "$OUT/attr.out" "coding-agent subprocesses: 1 procs, 200 MB total" "the agent aggregate counts the e2e agent alone"
    assert_silent_about "$OUT/attr.out" "pid=48000" "the user's own claude session is not reported"

    # The attributed sum against the compressor, with the gap.
    assert_says "$OUT/attr.out" "attributed 5.04 GB across 6 procs (phys_footprint) vs the instantaneous compressor 12.00 GB, gap 6.96 GB" "the attributed total and gap are stated against the instantaneous compressor"
}

# The global line may print the folded window peak while the attribution prints
# the instant. Both said "compressor" once, one line apart, and a first real run
# put 6.87 GB and 5.02 GB there. The labels are what keep that readable.
test_the_two_compressor_figures_at_one_boundary_are_labelled_apart() {
    echo "test: a folded peak and the attribution's instant are distinguishable"
    reset_state
    setup_attr_workspace 33660
    install_synthetic_attr_seams
    printf '1 12.00 30.00 0.00\n1 13.80 28.00 0.00\n' > "$HOST_MEMORY_SAMPLES_FILE"
    HOST_PRESSURE_LEVEL_OVERRIDE=1 HOST_COMPRESSOR_GB_OVERRIDE=12.00 \
        HOST_AVAIL_GB_OVERRIDE=30.00 HOST_SWAP_USED_GB_OVERRIDE=0.00 \
        HOST_PHYSMEM_GB_OVERRIDE=48 \
        check_host_memory_at_boundary "nav chunk 8/34" >"$OUT/two_figures.out" 2>&1
    assert_says "$OUT/two_figures.out" "compressor 13.80 GB, available 28.00 GB, swap 0.00 GB (worst of 2 samples)" "the global line carries the folded peak"
    assert_says "$OUT/two_figures.out" "vs the instantaneous compressor 12.00 GB" "the attribution names its reading as the instant"
}

test_attribution_engine_absent_from_top_n_is_still_reported() {
    echo "test: an e2e engine outside the top N is still reported explicitly"
    reset_state
    setup_attr_workspace 76951
    install_synthetic_attr_seams
    # 76951 is the SMALLER engine, well outside a top-1 list. It must still get its
    # own line, because its absence from the ranking is itself a finding.
    HOST_MEMORY_ATTR_TOP_N=1 HOST_COMPRESSOR_GB_OVERRIDE=12.00 HOST_AVAIL_GB_OVERRIDE=30.00 \
        HOST_SWAP_USED_GB_OVERRIDE=0.00 HOST_PHYSMEM_GB_OVERRIDE=48 \
        check_host_memory_at_boundary "nav chunk 6/10" >"$OUT/attr_eng.out" 2>&1
    assert_says "$OUT/attr_eng.out" "#1 com.apple.Virtualization.VirtualMachine pid=95531" "only the top 1 is ranked"
    assert_silent_about "$OUT/attr_eng.out" "#2 " "the ranking stopped at N=1"
    assert_says "$OUT/attr_eng.out" "e2e engine pid=76951 512 MB (primary suspect)" "the e2e engine is reported despite being outside the top N"
}

test_attribution_degrades_to_a_note_on_measurement_failure() {
    echo "test: a footprint that measures nothing degrades to a note, not an error"
    local rc
    reset_state
    setup_attr_workspace 33660
    install_synthetic_attr_seams
    # Candidates exist, but footprint refuses (root, or slow). The block must print
    # one note and let the run carry on, never an error and never a stop.
    # shellcheck disable=SC2329 # a seam: invoked by the sourced guard, not from this file
    _host_mem_attr_footprint() { return 0; }
    HOST_COMPRESSOR_GB_OVERRIDE=12.00 HOST_AVAIL_GB_OVERRIDE=30.00 \
        HOST_SWAP_USED_GB_OVERRIDE=0.00 HOST_PHYSMEM_GB_OVERRIDE=48 \
        check_host_memory_at_boundary "nav chunk 7/10" >"$OUT/attr_note.out" 2>&1
    rc=$?
    assert_eq "0" "$rc" "a measurement failure does not stop the run"
    assert_says "$OUT/attr_note.out" "[e2e-mem-attr] after nav chunk 7/10: no candidate footprint measured" "it prints one note explaining the skip"
    assert_silent_about "$OUT/attr_note.out" "STOP" "no stop was announced"
    assert_silent_about "$OUT/attr_note.out" "#1 " "no ranking is printed when nothing was measured"
}

test_attribution_does_not_change_the_stop_decision() {
    echo "test: the stop verdict is identical with the attribution block active"
    local rc
    reset_state
    setup_attr_workspace 33660
    install_synthetic_attr_seams
    # Swap over the limit stops the run. The attribution block runs first, but the
    # stop, its wording, and its exit code must be exactly what Test 2 pins.
    HOST_COMPRESSOR_GB_OVERRIDE=6.00 HOST_AVAIL_GB_OVERRIDE=30 \
        HOST_SWAP_USED_GB_OVERRIDE=3.50 HOST_PHYSMEM_GB_OVERRIDE=48 \
        check_host_memory_at_boundary "nav chunk 4/32" >"$OUT/attr_stop.out" 2>&1
    rc=$?
    assert_eq "1" "$rc" "the swap stop still returns non-zero with attribution active"
    assert_says "$OUT/attr_stop.out" "STOP: 3.50 GB of swap is in use" "the stop cause is unchanged"
    assert_says "$OUT/attr_stop.out" "over the 1 GB limit" "the stop limit is unchanged"
    # The block did run, so it is genuinely additive rather than absent.
    assert_says "$OUT/attr_stop.out" "[e2e-mem-attr] after nav chunk 4/32:" "the attribution block was emitted before the stop"
    case "$MEMORY_STOP_DETAIL" in
        *"nav chunk 4/32"*"3.50 GB of swap"*)
            pass "the recorded stop detail is unaffected by attribution" ;;
        *) fail "attribution changed the stop detail: '$MEMORY_STOP_DETAIL'" ;;
    esac
}

# A second synthetic host for the run-membership classification. Every process
# here shares a browser or node NAME with a run process, so a name-only
# classifier counts them all as the run. The executable path (under the
# Playwright browsers cache) and the cwd (under the e2e workspace) are what
# separate the run from the machine.
install_run_membership_seams() {
    # shellcheck disable=SC2329 # a seam: invoked by the sourced guard, not from this file
    _host_mem_attr_proc_list() {
        printf '%s\t%s\n' \
            30001 "/System/Library/Frameworks/WebKit.framework/Versions/A/XPCServices/com.apple.WebKit.WebContent.xpc/Contents/MacOS/com.apple.WebKit.WebContent" \
            30002 "/Users/me/Library/Caches/ms-playwright/webkit-2158/Playwright.app/Contents/MacOS/com.apple.WebKit.WebContent" \
            30003 "/Users/me/Library/Caches/ms-playwright/webkit-2158/Playwright.app/Contents/MacOS/com.apple.WebKit.WebContent" \
            30004 "/opt/homebrew/bin/node" \
            30005 "/opt/homebrew/bin/node"
    }
    # shellcheck disable=SC2329 # a seam: invoked by the sourced guard, not from this file
    _host_mem_attr_footprint() {
        cat <<'FP'
com.apple.WebKit.WebContent [30001]: 64-bit    Footprint: 536870912 B (16384 bytes per page)
com.apple.WebKit.WebContent [30002]: 64-bit    Footprint: 268435456 B (16384 bytes per page)
com.apple.WebKit.WebContent [30003]: 64-bit    Footprint: 134217728 B (16384 bytes per page)
node [30004]: 64-bit    Footprint: 104857600 B (16384 bytes per page)
node [30005]: 64-bit    Footprint: 209715200 B (16384 bytes per page)
FP
    }
    # 30005 is the e2e agent's node (cwd under the workspace); 30004 is the user's
    # own node, elsewhere. The WebKit pids never reach this seam: a browser is
    # classified by path, so the guard asks the kernel for no cwd on them.
    # shellcheck disable=SC2329 # a seam: invoked by the sourced guard, not from this file
    _host_mem_attr_cwd() {
        case "$1" in
            30005) printf '%s' "$E2E_WORKSPACE/.lucidos/worktrees/wt-b" ;;
            *) printf '%s' "/Users/me/projects/other" ;;
        esac
    }
}

# The false-positive class this change removes. A system WebKit content process
# and the user's own node share a name with the run's own. So the old name-only
# classifier counted both as the run. Path and cwd separate them now, and the
# stop decision is untouched by the reclassification.
test_attribution_classifies_by_run_membership_not_name() {
    echo "test: a system WebKit and a stray node are host demand; a Playwright one and an orphan are the run"
    local rc
    reset_state
    setup_attr_workspace 99999
    install_run_membership_seams
    HOST_COMPRESSOR_GB_OVERRIDE=12.00 HOST_AVAIL_GB_OVERRIDE=30.00 \
        HOST_SWAP_USED_GB_OVERRIDE=0.00 HOST_PHYSMEM_GB_OVERRIDE=48 \
        check_host_memory_at_boundary "nav chunk 8/10" >"$OUT/member.out" 2>&1
    rc=$?
    assert_eq "0" "$rc" "classification does not stop a healthy host"
    assert_silent_about "$OUT/member.out" "STOP" "the stop decision is unaffected"

    # A Playwright WebKit process (path under the browsers cache) is the run.
    assert_says "$OUT/member.out" "com.apple.WebKit.WebContent pid=30002 256 MB (browser)" "a Playwright WebKit process is a run browser"
    # An orphaned run browser (still under the cache) stays visible as the run.
    # This is why path beats ancestry: launchd re-parenting would hide it.
    assert_says "$OUT/member.out" "com.apple.WebKit.WebContent pid=30003 128 MB (browser)" "an orphaned run browser is still a run browser"
    # A system WebKit content process is host demand, never counted as the run.
    assert_says "$OUT/member.out" "com.apple.WebKit.WebContent pid=30001 512 MB (browser-host)" "a system WebKit process is host demand, not a run browser"
    assert_silent_about "$OUT/member.out" "pid=30001 512 MB (browser)" "the system WebKit process is not counted as a run browser"

    # A node with no cwd under the run is host demand; the agent's node is the run.
    assert_says "$OUT/member.out" "node pid=30004 100 MB (node-host)" "a stray node is host demand, not a run node"
    assert_silent_about "$OUT/member.out" "node pid=30004 100 MB (node)" "the stray node is not counted as a run node"
    assert_says "$OUT/member.out" "node pid=30005 200 MB (agent)" "the agent's node is the run"

    # Host demand stays in the attributed total, so it is never hidden from the
    # gap: 512 + 256 + 128 + 100 + 200 MB is 1.17 GB across all five procs.
    assert_says "$OUT/member.out" "attributed 1.17 GB across 5 procs" "host demand stays counted, not dropped"
}

test_the_old_ceiling_no_longer_stops_a_healthy_host
test_tonights_reading_runs_on
test_swap_over_the_limit_stops
test_swap_at_the_limit_does_not_stop
test_swap_wins_when_all_are_over
test_warn_pressure_corroborates_the_floor_and_stops
test_swap_in_use_corroborates_the_floor_and_stops
test_an_uncorroborated_reading_never_stops_however_deep
test_unreadable_pressure_corroborates_nothing
test_available_at_the_floor_does_not_stop
test_the_floor_scales_with_ram
test_the_floor_value_resolves_per_host
test_the_floor_uses_the_minimum_when_ram_is_unreadable
test_scarcity_wins_over_the_compressor_backstop
test_the_2026_07_26_wedge_stops_on_the_corroborated_floor
test_the_floor_is_never_below_the_preflight_gate
test_an_idle_host_reading_stops_neither_guard
test_a_corroborated_low_host_stops_both_guards
test_the_oscillating_idle_host_stops_neither_guard
test_the_freeze_reading_stops_both_guards
test_sustained_critical_stops_both_guards
test_a_garbage_confirm_knob_does_not_refuse_the_gate
test_the_runaway_backstop_scales_with_ram
test_critical_pressure_stops_an_otherwise_healthy_looking_host
test_a_single_critical_reading_at_the_boundary_does_not_stop
test_critical_with_the_headroom_gone_stops_without_waiting
test_critical_with_swap_in_use_stops_without_waiting
test_an_unreadable_level_mid_confirm_does_not_stop
test_the_confirm_window_knobs_fall_back_rather_than_disarming
test_the_collapse_level_scales_with_ram
test_warn_pressure_is_reported_but_never_stops
test_unreadable_pressure_never_stops
test_a_non_level_pressure_value_is_discarded
test_last_nights_boundary_now_runs_on
test_the_available_floor_false_stop_now_runs_on
test_the_recorded_freeze_reading_stops
test_a_cleared_critical_excursion_is_reported_and_not_acted_on
test_a_collapse_inside_the_chunk_stops_though_the_boundary_recovered
test_the_window_folds_worst_per_dimension
test_the_window_counts_under_floor_samples
test_the_window_counts_warn_or_worse_samples
test_a_zero_reading_in_the_window_is_a_value_not_an_absence
test_a_short_record_is_discarded_rather_than_read_as_zero
test_one_critical_tick_mid_chunk_does_not_stop
test_critical_still_standing_at_the_boundary_is_confirmed_before_it_stops
test_one_available_dip_mid_chunk_does_not_stop
test_one_available_dip_under_warn_pressure_still_does_not_stop
test_the_boundary_reading_alone_satisfies_the_sustain_arm
test_two_under_floor_samples_stop_even_after_the_boundary_recovered
test_a_single_warn_sample_does_not_corroborate
test_a_survived_dip_still_prints_the_window_minimum
test_an_unreadable_dimension_in_the_window_never_erases_the_direct_read
test_no_samples_falls_back_to_a_single_reading
test_the_sampler_starts_writes_and_is_reaped
test_the_sampler_never_signals_a_pid_it_did_not_spawn
test_a_collapse_streak_trips_the_chunk
test_only_the_freeze_signature_trips_mid_chunk
test_a_broken_streak_starts_over
test_the_trip_ticks_knob_falls_back
test_a_trip_interrupts_only_the_recorded_invocation
test_a_runner_that_ignores_the_interrupt_is_killed
test_a_trip_with_no_safe_runner_signals_nothing
test_an_empty_tree_feed_signals_only_the_runner
test_teardown_interrupts_only_a_runner_this_run_recorded
test_a_trip_while_the_runner_started_still_reaches_it
test_the_runner_record_and_the_trip_are_run_scoped
test_a_trip_is_read_back_as_a_memory_stop
test_a_boundary_after_a_trip_stops_on_it
test_the_reading_that_stopped_an_earlier_nightly_now_runs_on
test_explicit_absolute_ceiling_overrides_the_share
test_compressor_percent_knob_sets_the_runaway
test_garbage_knobs_fall_back_to_defaults
test_a_retired_cap_knob_is_named_rather_than_ignored
test_garbage_free_floor_knobs_fall_back
test_garbage_swap_knob_does_not_stop_a_host_with_some_swap
test_negative_percent_cannot_stop_everything
test_unreadable_host_fails_open
test_unreadable_ram_drops_the_compressor_backstop_but_keeps_swap
test_swap_units_are_read_off_the_value
test_compressor_uses_the_reported_page_size
test_available_sums_free_speculative_purgeable_filebacked
test_start_records_the_baseline_and_states_the_thresholds
test_the_swap_stop_calls_itself_measured_distress
test_the_compressor_stop_says_the_host_was_never_in_trouble
test_the_backstop_stop_does_not_clear_a_host_it_could_not_read
test_stop_report_is_silent_on_a_run_that_finished
test_stop_report_states_what_this_run_itself_cost
test_stop_report_does_not_advise_raising_the_ceiling
test_stop_exit_code_is_the_os_error_code
test_attribution_block_is_emitted
test_the_two_compressor_figures_at_one_boundary_are_labelled_apart
test_attribution_engine_absent_from_top_n_is_still_reported
test_attribution_degrades_to_a_note_on_measurement_failure
test_attribution_does_not_change_the_stop_decision
test_attribution_classifies_by_run_membership_not_name

echo ""
echo "Passed: $PASS  Failed: $FAIL  Skipped: $SKIPPED"
# A skip is never a pass. It is counted and named so a run that could not reach
# the pre-flight gate says so, rather than reading as full coverage.
if [ "$SKIPPED" -gt 0 ]; then
    echo "NOTE: $SKIPPED assertion group(s) skipped. The gate half of the agreement was not verified on this host."
fi
[ "$FAIL" -eq 0 ]
