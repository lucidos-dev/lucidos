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

assert_says() {
    local file="$1" needle="$2" msg="$3"
    if grep -qF "$needle" "$file"; then
        pass "$msg"
    else
        fail "$msg (no '$needle' in output)"
        sed 's/^/      | /' "$file"
    fi
}

assert_silent_about() {
    local file="$1" needle="$2" msg="$3"
    if grep -qF "$needle" "$file"; then
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
    # A window left behind by the peak-sampler tests would be folded into the next
    # test's boundary reading, which is exactly the cross-test leak reset_state
    # exists to prevent.
    rm -f "$HOST_MEMORY_SAMPLES_FILE" "$HOST_MEMORY_SAMPLER_PIDFILE" 2>/dev/null || true
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
    assert_silent_about "$OUT/both.out" "REAL SCARCITY" "the scarcity message is not also printed"
}

# ── Test 3: the free-headroom floor ─────────────────────────────────────────
test_available_floor_stops_a_low_host() {
    echo "test: available memory under the floor stops the run"
    local rc
    reset_state
    # Compressor and swap both fine, so only the floor can be what stopped it.
    HOST_COMPRESSOR_GB_OVERRIDE=8.00 HOST_AVAIL_GB_OVERRIDE=3.00 \
        HOST_SWAP_USED_GB_OVERRIDE=0.00 HOST_PHYSMEM_GB_OVERRIDE=48 \
        check_host_memory_at_boundary "nav chunk 15/32" >"$OUT/scarce.out" 2>&1
    rc=$?
    assert_eq "1" "$rc" "available under the floor returns non-zero"
    assert_says "$OUT/scarce.out" "STOP: available memory 3.00 GB is under the 9.60 GB floor" "the stop names the floor it broke"
    assert_says "$OUT/scarce.out" "REAL SCARCITY" "the stop names itself as scarcity"
    case "$MEMORY_STOP_DETAIL" in
        *"nav chunk 15/32"*"available memory was 3.00 GB"*) pass "the detail records the boundary and reading" ;;
        *) fail "detail did not record the boundary and reading: '$MEMORY_STOP_DETAIL'" ;;
    esac
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
    local rc
    reset_state
    HOST_COMPRESSOR_GB_OVERRIDE=5.00 HOST_AVAIL_GB_OVERRIDE=9.00 \
        HOST_SWAP_USED_GB_OVERRIDE=0.00 HOST_PHYSMEM_GB_OVERRIDE=48 \
        check_host_memory_at_boundary "nav chunk 9/32" >"$OUT/floorbig.out" 2>&1
    rc=$?
    assert_eq "1" "$rc" "9 GB available on a 48 GB host stops"
    assert_says "$OUT/floorbig.out" "under the 9.60 GB floor" "the floor is a share of 48 GB RAM"

    reset_state
    HOST_COMPRESSOR_GB_OVERRIDE=5.00 HOST_AVAIL_GB_OVERRIDE=9.00 \
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

    HOST_COMPRESSOR_GB_OVERRIDE=9.00 HOST_AVAIL_GB_OVERRIDE=3.00 \
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
    HOST_COMPRESSOR_GB_OVERRIDE=30.00 HOST_AVAIL_GB_OVERRIDE=2.00 \
        HOST_SWAP_USED_GB_OVERRIDE=0.00 HOST_PHYSMEM_GB_OVERRIDE=48 \
        check_host_memory_at_boundary "nav chunk 28/34" >"$OUT/scarcewins.out" 2>&1
    rc=$?
    assert_eq "1" "$rc" "both over returns non-zero"
    assert_says "$OUT/scarcewins.out" "REAL SCARCITY" "scarcity is reported as the cause"
    assert_silent_about "$OUT/scarcewins.out" "RUNAWAY BACKSTOP" "the runaway message is not also printed"
    assert_silent_about "$OUT/scarcewins.out" "SURVIVABILITY" "the cap message is not also printed"
}

# The 2026-07-26 wedge reading. That night the compressor hit 17.41 GB with swap
# still clear. This asserts that when available is also scarce at that reading, the
# guard stops on the floor and names scarcity, ahead of the compressor cap and the
# runaway backstop.
test_the_2026_07_26_wedge_stops_on_the_floor() {
    echo "test: compressor 17.41 GB, swap 0, available scarce stops on the floor"
    local rc
    reset_state
    HOST_COMPRESSOR_GB_OVERRIDE=17.41 HOST_AVAIL_GB_OVERRIDE=5.00 \
        HOST_SWAP_USED_GB_OVERRIDE=0.00 HOST_PHYSMEM_GB_OVERRIDE=48 \
        check_host_memory_at_boundary "nav chunk 30/34" >"$OUT/wedge.out" 2>&1
    rc=$?
    assert_eq "1" "$rc" "the wedge reading stops when available is scarce"
    assert_says "$OUT/wedge.out" "STOP: available memory 5.00 GB is under the 9.60 GB floor" "the floor is what stopped it"
    assert_says "$OUT/wedge.out" "REAL SCARCITY" "scarcity is named as the cause"
    assert_silent_about "$OUT/wedge.out" "SURVIVABILITY" "the cap message did not also fire (scarcity is first)"
    assert_silent_about "$OUT/wedge.out" "RUNAWAY BACKSTOP" "the runaway message did not also fire"
}

# The in-run floor and the pre-flight gate must never disagree. The gate refuses to
# START a run under 8 GB available (AVAILABLE_MIN_GB), so the in-run floor must
# never drop below 8, or the running guard would continue on a host the gate would
# have refused. max(8, 20% of RAM) guarantees it on every host size.
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

# ── Test 5: the kernel's own verdict is the danger stop ──────────────────────
# CRITICAL is the freeze signature: the one recorded freeze on this host read
# compressor 17.41 GB, free 0.04 GB, pressure critical. Compressor size did not
# distinguish it from a healthy night; pressure did.
test_critical_pressure_stops_an_otherwise_healthy_looking_host() {
    echo "test: kernel pressure critical stops, even with every other reading fine"
    local rc
    reset_state
    HOST_PRESSURE_LEVEL_OVERRIDE=4 HOST_COMPRESSOR_GB_OVERRIDE=6.00 \
        HOST_AVAIL_GB_OVERRIDE=30.00 HOST_SWAP_USED_GB_OVERRIDE=0.00 \
        HOST_PHYSMEM_GB_OVERRIDE=48 \
        check_host_memory_at_boundary "nav chunk 9/34" >"$OUT/crit.out" 2>&1
    rc=$?
    assert_eq "1" "$rc" "critical pressure stops the run"
    assert_says "$OUT/crit.out" "CRITICAL memory pressure" "the stop names the pressure level"
    assert_says "$OUT/crit.out" "KERNEL'S OWN VERDICT" "the stop says why pressure is the instrument"
    assert_says "$OUT/crit.out" "pressure critical," "the boundary line states the level"
    assert_silent_about "$OUT/crit.out" "RUNAWAY" "the compressor did not also fire"
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

# The freeze itself, for contrast, on the same instrument. Compressor 17.41 GB is
# 0.30 GB from the healthy reading above; pressure and available are what differ.
test_the_recorded_freeze_reading_stops() {
    echo "test: the recorded freeze reading (critical, 0.04 GB free) stops"
    local rc
    reset_state
    HOST_PRESSURE_LEVEL_OVERRIDE=4 HOST_COMPRESSOR_GB_OVERRIDE=17.41 \
        HOST_AVAIL_GB_OVERRIDE=0.30 HOST_SWAP_USED_GB_OVERRIDE=0.00 \
        HOST_PHYSMEM_GB_OVERRIDE=48 \
        check_host_memory_at_boundary "nav chunk 33/34" >"$OUT/freeze.out" 2>&1
    rc=$?
    assert_eq "1" "$rc" "the freeze reading stops"
    assert_says "$OUT/freeze.out" "CRITICAL memory pressure" "pressure is what caught it, ahead of the floor"
}

# ── Test 7: the boundary judges the whole chunk, not the instant ─────────────
# A boundary sample bounds the reading at the boundary and says nothing about the
# chunk that just ran. The sampler's window is folded over it, worst per
# dimension, so an excursion that came and went is still seen.
test_a_critical_excursion_inside_the_chunk_is_seen_at_the_boundary() {
    echo "test: a critical sample mid-chunk stops even when the boundary reads healthy"
    local rc
    reset_state
    printf '1 6.00 30.00 0.00\n4 9.00 11.00 0.00\n4 9.10 11.20 0.00\n' > "$HOST_MEMORY_SAMPLES_FILE"
    HOST_PRESSURE_LEVEL_OVERRIDE=1 HOST_COMPRESSOR_GB_OVERRIDE=6.30 \
        HOST_AVAIL_GB_OVERRIDE=30.00 HOST_SWAP_USED_GB_OVERRIDE=0.00 \
        HOST_PHYSMEM_GB_OVERRIDE=48 \
        check_host_memory_at_boundary "nav chunk 7/34" >"$OUT/peak.out" 2>&1
    rc=$?
    assert_eq "1" "$rc" "the mid-chunk critical excursion stops the run"
    assert_says "$OUT/peak.out" "worst of 3 samples" "the line says it judged the window"
    assert_says "$OUT/peak.out" "CRITICAL memory pressure" "the excursion is what stopped it"
}

test_the_window_folds_worst_per_dimension() {
    echo "test: the window takes the highest pressure, compressor and swap, the lowest available"
    local got
    reset_state
    printf '1 6.00 30.00 0.00\n2 9.50 11.00 0.25\n1 7.00 22.00 0.10\n' > "$HOST_MEMORY_SAMPLES_FILE"
    got="$(_host_mem_window_worst)"
    assert_eq "2 9.50 11.00 0.25 0 3" "$got" "worst is max level, max compressor, min available, max swap"
    # Reading TRUNCATES, so the next boundary judges its own chunk rather than
    # inheriting this one's peak forever.
    got="$(_host_mem_window_worst)"
    assert_eq "" "$got" "a second read sees an empty window"
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
    assert_eq "1 6.00 30.00 0.00 0 1" "$got" "the blank line is dropped, not folded in as zeros"

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
    assert_silent_about "$OUT/onetick.out" "STOP:" "no stop was announced"
}

test_critical_still_standing_at_the_boundary_stops_on_its_own() {
    echo "test: critical at the boundary itself stops, with no window at all"
    local rc
    reset_state
    HOST_PRESSURE_LEVEL_OVERRIDE=4 HOST_COMPRESSOR_GB_OVERRIDE=6.00 \
        HOST_AVAIL_GB_OVERRIDE=30.00 HOST_SWAP_USED_GB_OVERRIDE=0.00 \
        HOST_PHYSMEM_GB_OVERRIDE=48 \
        check_host_memory_at_boundary "nav chunk 6/34" >"$OUT/critnow.out" 2>&1
    rc=$?
    assert_eq "1" "$rc" "critical at the boundary stops without needing a window"
    assert_says "$OUT/critnow.out" "CRITICAL memory pressure" "the stop names the pressure level"
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
    assert_eq "1 0.00 29.00 0.00 0 2" "$got" "zero compressor and zero swap survive the fold"
}

test_an_unreadable_dimension_in_the_window_never_erases_the_direct_read() {
    echo "test: a '-' dimension in the window leaves the boundary's own reading standing"
    local rc
    reset_state
    # Every sample failed to read available memory. The direct read did not, and
    # it is under the floor, so the floor must still fire.
    printf '1 6.00 - 0.00\n1 6.10 - 0.00\n' > "$HOST_MEMORY_SAMPLES_FILE"
    HOST_PRESSURE_LEVEL_OVERRIDE=1 HOST_COMPRESSOR_GB_OVERRIDE=6.10 \
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
    # Garbage knobs must resolve to the 9.6 GB default floor, so 9 GB available stops.
    HOST_COMPRESSOR_GB_OVERRIDE=6.00 HOST_AVAIL_GB_OVERRIDE=9.00 \
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
        "stops at: kernel pressure critical (the freeze signature), swap over 1 GB (distress), available under 9.60 GB (scarcity), or compressor over 24.00 GB (runaway backstop)." \
        "the four thresholds are stated up front, and labelled"
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
    assert_silent_about "$OUT/distress.out" "REAL SCARCITY" "it does not also claim scarcity"
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
test_available_floor_stops_a_low_host
test_available_at_the_floor_does_not_stop
test_the_floor_scales_with_ram
test_the_floor_value_resolves_per_host
test_the_floor_uses_the_minimum_when_ram_is_unreadable
test_scarcity_wins_over_the_compressor_backstop
test_the_2026_07_26_wedge_stops_on_the_floor
test_the_floor_is_never_below_the_preflight_gate
test_the_runaway_backstop_scales_with_ram
test_critical_pressure_stops_an_otherwise_healthy_looking_host
test_warn_pressure_is_reported_but_never_stops
test_unreadable_pressure_never_stops
test_a_non_level_pressure_value_is_discarded
test_last_nights_boundary_now_runs_on
test_the_recorded_freeze_reading_stops
test_a_critical_excursion_inside_the_chunk_is_seen_at_the_boundary
test_the_window_folds_worst_per_dimension
test_a_zero_reading_in_the_window_is_a_value_not_an_absence
test_a_short_record_is_discarded_rather_than_read_as_zero
test_one_critical_tick_mid_chunk_does_not_stop
test_critical_still_standing_at_the_boundary_stops_on_its_own
test_an_unreadable_dimension_in_the_window_never_erases_the_direct_read
test_no_samples_falls_back_to_a_single_reading
test_the_sampler_starts_writes_and_is_reaped
test_the_sampler_never_signals_a_pid_it_did_not_spawn
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
echo "Passed: $PASS  Failed: $FAIL"
[ "$FAIL" -eq 0 ]
