#!/bin/bash
# Tests for scripts/lib/host_memory_guard.sh, the host-memory stop condition that
# ends a browser e2e run at a chunk boundary when the host is out of memory to lend.
# Run: ./scripts/lib/host_memory_guard_test.sh   (no harness; direct, like host_load_guard_test.sh)
#
# Hermetic: every reading is injected through the HOST_COMPRESSOR_GB_OVERRIDE /
# HOST_SWAP_USED_GB_OVERRIDE / HOST_PHYSMEM_GB_OVERRIDE seams, never a real
# vm_stat or sysctl. The two cases that exercise PARSING instead of policy shadow
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
}

# ── Test 1: the regression. A busy host that used to be stopped now runs on ──
test_the_old_ceiling_no_longer_stops_a_healthy_host() {
    echo "test: 12.25 GB compressor with no swap on a 48 GB host runs on"
    # This is the exact reading that ended the unfiltered mobile-webkit project at
    # nav chunk 20 of 32 with exit 71, zero test failures and zero reaper kills.
    # Swap was 0.00M and pressure was normal throughout, so the host was fine.
    local rc
    reset_state
    HOST_COMPRESSOR_GB_OVERRIDE=12.25 HOST_SWAP_USED_GB_OVERRIDE=0.00 \
        HOST_PHYSMEM_GB_OVERRIDE=48 \
        check_host_memory_at_boundary "nav chunk 20/32" >"$OUT/reg.out" 2>&1
    rc=$?
    assert_eq "0" "$rc" "the run continues past the old 12 GB ceiling"
    assert_says "$OUT/reg.out" "compressor 12.25 GB, swap 0.00 GB" "the boundary line states both numbers"
    assert_silent_about "$OUT/reg.out" "STOP" "no stop was announced"
}

# ── Test 2: swap in use is the real stop condition ──────────────────────────
test_swap_over_the_limit_stops() {
    echo "test: swap over the limit stops the run, whatever the compressor says"
    local rc
    reset_state
    # A modest compressor, so only swap can be what stopped it.
    HOST_COMPRESSOR_GB_OVERRIDE=6.00 HOST_SWAP_USED_GB_OVERRIDE=3.50 \
        HOST_PHYSMEM_GB_OVERRIDE=48 \
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
    HOST_COMPRESSOR_GB_OVERRIDE=6.00 HOST_SWAP_USED_GB_OVERRIDE=1.00 \
        HOST_PHYSMEM_GB_OVERRIDE=48 LUCIDOS_E2E_SWAP_MAX_GB=1 \
        check_host_memory_at_boundary "nav chunk 5/32" >"$OUT/swapeq.out" 2>&1
    rc=$?
    assert_eq "0" "$rc" "swap equal to the limit returns 0"
}

test_swap_wins_when_both_are_over() {
    echo "test: with swap and compressor both over, the message names swap"
    local rc
    reset_state
    HOST_COMPRESSOR_GB_OVERRIDE=40.00 HOST_SWAP_USED_GB_OVERRIDE=9.00 \
        HOST_PHYSMEM_GB_OVERRIDE=48 \
        check_host_memory_at_boundary "cc chunk 1/4" >"$OUT/both.out" 2>&1
    rc=$?
    assert_eq "1" "$rc" "both over returns non-zero"
    assert_says "$OUT/both.out" "STOP: 9.00 GB of swap is in use" "swap is reported as the cause"
    assert_silent_about "$OUT/both.out" "backstop" "the backstop message is not also printed"
}

# ── Test 3: the compressor backstop scales with physical memory ─────────────
test_backstop_scales_with_ram() {
    echo "test: the same compressor reading stops a 16 GB host and not a 48 GB one"
    # A fixed 12 GB ceiling meant 75% of one machine and 25% of the other, which is
    # why it could not be calibrated. The share is what keeps a SMALL host honest,
    # and 13 GB is the reading that separates the two. The 16 GB cap is the other
    # half of the resolution and never binds at this reading.
    local rc
    reset_state
    HOST_COMPRESSOR_GB_OVERRIDE=13.00 HOST_SWAP_USED_GB_OVERRIDE=0.00 \
        HOST_PHYSMEM_GB_OVERRIDE=16 \
        check_host_memory_at_boundary "nav chunk 9/32" >"$OUT/small.out" 2>&1
    rc=$?
    assert_eq "1" "$rc" "13 GB compressed on a 16 GB host stops"
    assert_says "$OUT/small.out" "over the 8.00 GB backstop" "the backstop is half of 16 GB"
    assert_says "$OUT/small.out" "Swap is still clear" "the message says this is the backstop, not distress"

    reset_state
    HOST_COMPRESSOR_GB_OVERRIDE=13.00 HOST_SWAP_USED_GB_OVERRIDE=0.00 \
        HOST_PHYSMEM_GB_OVERRIDE=48 \
        check_host_memory_at_boundary "nav chunk 9/32" >"$OUT/big.out" 2>&1
    rc=$?
    assert_eq "0" "$rc" "13 GB compressed on a 48 GB host runs on"
}

# The other half of the same rule, and the half that was missing. A share alone
# put this 48 GB host's backstop at 24.00 GB, which no run has ever reached, so
# the only ceiling in force was whatever the caller exported.
test_the_cap_bites_before_the_share_on_a_large_host() {
    echo "test: on a 48 GB host the 16 GB cap is the backstop, not the 24 GB share"
    local rc got
    reset_state
    got="$(HOST_PHYSMEM_GB_OVERRIDE=48 _host_mem_compressor_ceiling_gb)"
    assert_eq "16.00" "$got" "48 GB resolves to the cap, not half of RAM"

    got="$(HOST_PHYSMEM_GB_OVERRIDE=16 _host_mem_compressor_ceiling_gb)"
    assert_eq "8.00" "$got" "16 GB still resolves to the share, which is lower"

    HOST_COMPRESSOR_GB_OVERRIDE=20.00 HOST_SWAP_USED_GB_OVERRIDE=0.00 \
        HOST_PHYSMEM_GB_OVERRIDE=48 \
        check_host_memory_at_boundary "nav chunk 22/33" >"$OUT/cap.out" 2>&1
    rc=$?
    assert_eq "1" "$rc" "20 GB compressed on a 48 GB host now stops"
    assert_says "$OUT/cap.out" "over the 16.00 GB backstop" "the cap is the number it names"
}

# 14.98 GB is where the 2026-08-31 nightly stopped, on the ceiling its spawn
# intent exported. The checked-in default must not stop there once nobody sets
# one, or the phase reversal buys nothing.
test_the_reading_that_stopped_the_nightly_now_runs_on() {
    echo "test: 14.98 GB with no exported ceiling runs on"
    local rc
    reset_state
    HOST_COMPRESSOR_GB_OVERRIDE=14.98 HOST_SWAP_USED_GB_OVERRIDE=0.25 \
        HOST_PHYSMEM_GB_OVERRIDE=48 \
        check_host_memory_at_boundary "mobile-webkit phase 1/2 (CC)" >"$OUT/nightly.out" 2>&1
    rc=$?
    assert_eq "0" "$rc" "the boundary that ended the nightly no longer stops"
    assert_silent_about "$OUT/nightly.out" "STOP" "no stop was announced"
}

test_explicit_absolute_ceiling_overrides_the_share() {
    echo "test: LUCIDOS_E2E_COMPRESSOR_MAX_GB overrides the RAM share"
    local rc
    reset_state
    HOST_COMPRESSOR_GB_OVERRIDE=10.00 HOST_SWAP_USED_GB_OVERRIDE=0.00 \
        HOST_PHYSMEM_GB_OVERRIDE=48 LUCIDOS_E2E_COMPRESSOR_MAX_GB=9 \
        check_host_memory_at_boundary "nav chunk 2/32" >"$OUT/abs.out" 2>&1
    rc=$?
    assert_eq "1" "$rc" "an explicit lower ceiling still stops the run"
    assert_says "$OUT/abs.out" "over the 9 GB backstop" "the explicit value is the one applied"

    # It replaces BOTH defaults, not just the share. An operator who names a
    # number above the cap means that number, and silently capping it back to 16
    # would make an exported ceiling a lie.
    local got
    got="$(HOST_PHYSMEM_GB_OVERRIDE=48 LUCIDOS_E2E_COMPRESSOR_MAX_GB=20 \
        _host_mem_compressor_ceiling_gb)"
    assert_eq "20" "$got" "an explicit ceiling above the cap is honored, not clamped"
}

test_percent_knob_is_honored() {
    echo "test: LUCIDOS_E2E_COMPRESSOR_MAX_PCT moves the backstop below the cap"
    local rc
    reset_state
    HOST_COMPRESSOR_GB_OVERRIDE=20.00 HOST_SWAP_USED_GB_OVERRIDE=0.00 \
        HOST_PHYSMEM_GB_OVERRIDE=48 LUCIDOS_E2E_COMPRESSOR_MAX_PCT=25 \
        check_host_memory_at_boundary "nav chunk 3/32" >"$OUT/pct.out" 2>&1
    rc=$?
    assert_eq "1" "$rc" "25% of 48 GB stops a 20 GB compressor"
    assert_says "$OUT/pct.out" "over the 12.00 GB backstop" "the backstop is a quarter of RAM"
}

# The knob moves the SHARE, and the lower of share and cap still wins. So above
# the cap it is a no-op, which the knobs comment now says in words. Without this
# case the whole clamped region is untested and an operator setting 75 could
# quietly get 16 with nothing to point at.
test_the_percent_knob_cannot_raise_the_backstop_past_the_cap() {
    echo "test: a percentage above the cap is clamped, not honored"
    local got
    reset_state
    got="$(HOST_PHYSMEM_GB_OVERRIDE=48 LUCIDOS_E2E_COMPRESSOR_MAX_PCT=75 \
        _host_mem_compressor_ceiling_gb)"
    assert_eq "16.00" "$got" "75% of 48 GB is clamped to the cap, not 36.00"

    # The escape is naming a number in GB, which the knobs comment points at.
    got="$(HOST_PHYSMEM_GB_OVERRIDE=48 LUCIDOS_E2E_COMPRESSOR_MAX_PCT=75 \
        LUCIDOS_E2E_COMPRESSOR_MAX_GB=36 _host_mem_compressor_ceiling_gb)"
    assert_eq "36" "$got" "an absolute ceiling is the way past the cap"
}

# ── Test 4: a garbage knob must never stop the suite ────────────────────────
test_garbage_knobs_fall_back_to_defaults() {
    echo "test: unusable overrides fall back to the defaults rather than stopping"
    local rc
    reset_state
    HOST_COMPRESSOR_GB_OVERRIDE=13.00 HOST_SWAP_USED_GB_OVERRIDE=0.00 \
        HOST_PHYSMEM_GB_OVERRIDE=48 \
        LUCIDOS_E2E_COMPRESSOR_MAX_GB=lots LUCIDOS_E2E_COMPRESSOR_MAX_PCT=-4 \
        LUCIDOS_E2E_SWAP_MAX_GB=banana \
        check_host_memory_at_boundary "nav chunk 1/32" >"$OUT/junk.out" 2>&1
    rc=$?
    assert_eq "0" "$rc" "garbage knobs do not stop a healthy host"
    assert_says "$OUT/junk.out" "compressor 13.00 GB" "the boundary still reported the reading"
}

# The case above cannot see a broken swap ceiling, because it injects zero swap
# and zero is over no limit. This one injects real swap, so a garbage ceiling
# coerced to zero would stop the run at the first boundary.
test_garbage_swap_knob_does_not_stop_a_host_with_some_swap() {
    echo "test: an unusable swap knob falls back to 1 GB, not to zero"
    local rc
    reset_state
    HOST_COMPRESSOR_GB_OVERRIDE=5.00 HOST_SWAP_USED_GB_OVERRIDE=0.25 \
        HOST_PHYSMEM_GB_OVERRIDE=48 LUCIDOS_E2E_SWAP_MAX_GB=banana \
        check_host_memory_at_boundary "nav chunk 1/32" >"$OUT/swapjunk.out" 2>&1
    rc=$?
    assert_eq "0" "$rc" "0.25 GB of swap is under the default 1 GB ceiling"
    assert_silent_about "$OUT/swapjunk.out" "STOP" "the boundary did not stop the run"
}

test_negative_percent_cannot_stop_everything() {
    echo "test: a zero-percent backstop falls back rather than stopping at once"
    local rc
    reset_state
    HOST_COMPRESSOR_GB_OVERRIDE=1.00 HOST_SWAP_USED_GB_OVERRIDE=0.00 \
        HOST_PHYSMEM_GB_OVERRIDE=48 LUCIDOS_E2E_COMPRESSOR_MAX_PCT=0 \
        check_host_memory_at_boundary "nav chunk 1/32" >"$OUT/zero.out" 2>&1
    rc=$?
    assert_eq "0" "$rc" "a 0% backstop is rejected, so 1 GB passes"
}

# ── Test 5: fail open when the host cannot be measured ──────────────────────
test_unreadable_host_fails_open() {
    echo "test: an unmeasurable host skips the check instead of stopping"
    local rc
    reset_state
    # Shadow both host commands so every reader comes back empty. A guard that
    # cannot measure must never end a run.
    # shellcheck disable=SC2329 # a seam: invoked by the sourced guard, not from this file
    vm_stat() { return 1; }
    # shellcheck disable=SC2329 # a seam: invoked by the sourced guard, not from this file
    sysctl() { return 1; }
    check_host_memory_at_boundary "nav chunk 7/32" >"$OUT/blind.out" 2>&1
    rc=$?
    unset -f vm_stat sysctl
    assert_eq "0" "$rc" "an unreadable host returns 0"
    assert_says "$OUT/blind.out" "host memory unreadable, check skipped" "it says why it skipped"
}

test_unreadable_ram_drops_the_backstop_but_keeps_swap() {
    echo "test: with RAM unreadable the backstop lapses and swap still stops the run"
    local rc
    reset_state
    # No physical-memory reading and no explicit ceiling, so the share cannot be
    # computed. A huge compressor must then pass, while swap still bites.
    # shellcheck disable=SC2329 # a seam: invoked by the sourced guard, not from this file
    sysctl() { case "$2" in hw.memsize) return 1 ;; *) return 1 ;; esac; }
    HOST_COMPRESSOR_GB_OVERRIDE=99.00 HOST_SWAP_USED_GB_OVERRIDE=0.00 \
        check_host_memory_at_boundary "nav chunk 8/32" >"$OUT/noram.out" 2>&1
    rc=$?
    assert_eq "0" "$rc" "no computable backstop means the compressor cannot stop the run"

    reset_state
    HOST_COMPRESSOR_GB_OVERRIDE=99.00 HOST_SWAP_USED_GB_OVERRIDE=4.00 \
        check_host_memory_at_boundary "nav chunk 8/32" >"$OUT/noram2.out" 2>&1
    rc=$?
    unset -f sysctl
    assert_eq "1" "$rc" "swap still stops the run with no backstop available"
}

# ── Test 6: swap parsing, the one place a unit mistake is silent ────────────
test_swap_units_are_read_off_the_value() {
    echo "test: vm.swapusage is parsed in M, G and K"
    local got
    # A host that has never swapped. Reading 0.00M as 0 GB is what keeps a healthy
    # run going; reading it as 0 megabytes-worth-of-gigabytes would too, so the
    # case that matters is the G one below.
    sysctl() { echo "total = 0.00M  used = 0.00M  free = 0.00M  (encrypted)"; }
    got="$(_host_mem_read_swap_used_gb)"
    assert_eq "0.00" "$got" "0.00M reads as 0.00 GB"

    # The shape of the recorded pile-up: gigabytes, which must not be divided.
    sysctl() { echo "total = 16384.00M  used = 14336.00M  free = 2048.00M  (encrypted)"; }
    got="$(_host_mem_read_swap_used_gb)"
    assert_eq "14.00" "$got" "14336.00M reads as 14.00 GB"

    sysctl() { echo "total = 20.00G  used = 2.50G  free = 17.50G  (encrypted)"; }
    got="$(_host_mem_read_swap_used_gb)"
    assert_eq "2.50" "$got" "2.50G reads as 2.50 GB, not 0.00"

    sysctl() { echo "total = 512.00K  used = 512.00K  free = 0.00K  (encrypted)"; }
    got="$(_host_mem_read_swap_used_gb)"
    assert_eq "0.00" "$got" "512.00K reads as effectively no swap"
    unset -f sysctl
}

test_compressor_uses_the_reported_page_size() {
    echo "test: vm_stat's own page size is used, not a hardcoded 4 KB"
    local got
    # Apple silicon reports 16384. 65536 pages at 16 KB is exactly 1 GB; read with
    # a 4 KB constant it would come back as 0.25 and never trip anything.
    vm_stat() {
        echo "Mach Virtual Memory Statistics: (page size of 16384 bytes)"
        echo "Pages occupied by compressor:              65536."
    }
    got="$(_host_mem_read_compressor_gb)"
    assert_eq "1.00" "$got" "65536 pages of 16 KB reads as 1.00 GB"
    unset -f vm_stat
}

# ── Test 7: the reports ─────────────────────────────────────────────────────
test_start_records_the_baseline_and_states_the_thresholds() {
    echo "test: the start line records the baseline and names both thresholds"
    reset_state
    HOST_COMPRESSOR_GB_OVERRIDE=4.39 HOST_SWAP_USED_GB_OVERRIDE=0.00 \
        HOST_PHYSMEM_GB_OVERRIDE=48 \
        report_host_memory_start >"$OUT/start.out" 2>&1
    assert_eq "4.39" "$HOST_MEMORY_BASELINE_GB" "the baseline was recorded"
    assert_says "$OUT/start.out" "browser phase start: compressor 4.39 GB, swap 0.00 GB" "the start line states both readings"
    assert_says "$OUT/start.out" \
        "stops at: swap over 1 GB (distress), or compressor over 16.00 GB (backstop)." \
        "the thresholds are stated up front, and labelled"
}

# ── Test 7b: the two stops must not read alike ──────────────────────────
# This pair is the whole point of the wording. A reader at 06:30 decides from
# these lines whether the Mac was in trouble, and "Swap is still clear, so this
# is the runaway backstop" was too easy to skim past.
test_the_swap_stop_calls_itself_measured_distress() {
    echo "test: the swap stop says the host is in trouble, in plain words"
    reset_state
    HOST_COMPRESSOR_GB_OVERRIDE=9.00 HOST_SWAP_USED_GB_OVERRIDE=2.40 \
        HOST_PHYSMEM_GB_OVERRIDE=48 \
        check_host_memory_at_boundary "nav chunk 12/33" >"$OUT/distress.out" 2>&1
    assert_says "$OUT/distress.out" "MEASURED DISTRESS" "the swap stop names itself as distress"
    assert_says "$OUT/distress.out" "the host is in" "it says the host is in trouble"
    assert_silent_about "$OUT/distress.out" "BACKSTOP" "it does not also claim to be the backstop"
    case "$MEMORY_STOP_DETAIL" in
        *"measured distress"*) pass "the final-verdict detail carries the classification" ;;
        *) fail "detail did not classify the stop: '$MEMORY_STOP_DETAIL'" ;;
    esac
}

test_the_compressor_stop_says_the_host_was_never_in_trouble() {
    echo "test: the backstop stop quotes the swap reading that proves it"
    reset_state
    HOST_COMPRESSOR_GB_OVERRIDE=16.40 HOST_SWAP_USED_GB_OVERRIDE=0.25 \
        HOST_PHYSMEM_GB_OVERRIDE=48 \
        check_host_memory_at_boundary "nav chunk 30/33" >"$OUT/runaway.out" 2>&1
    assert_says "$OUT/runaway.out" "RUNAWAY BACKSTOP, NOT distress" "the stop names itself as the backstop"
    assert_says "$OUT/runaway.out" "Swap is still clear at" "it states the swap reading, not just the word clear"
    assert_says "$OUT/runaway.out" "0.25 GB, under its 1 GB limit" "the reading and the limit it cleared are both named"
    assert_silent_about "$OUT/runaway.out" "MEASURED DISTRESS" "it does not also claim distress"
    case "$MEMORY_STOP_DETAIL" in
        *"not in distress"*) pass "the final-verdict detail carries the classification" ;;
        *) fail "detail did not classify the stop: '$MEMORY_STOP_DETAIL'" ;;
    esac
}

# "Not distress" is a claim about SWAP, so it cannot be made when swap was not
# read. The compressor comes from vm_stat and swap from sysctl, so one can fail
# alone, and the boundary only skips the check when BOTH are unreadable.
test_the_backstop_stop_does_not_clear_a_host_it_could_not_read() {
    echo "test: with swap unreadable the backstop stop refuses to say the host was fine"
    local rc
    reset_state
    # shellcheck disable=SC2329 # a seam: invoked by the sourced guard, not from this file
    sysctl() { return 1; }
    HOST_COMPRESSOR_GB_OVERRIDE=20.00 HOST_PHYSMEM_GB_OVERRIDE=48 \
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
    HOST_COMPRESSOR_GB_OVERRIDE=4.39 HOST_SWAP_USED_GB_OVERRIDE=0.00 \
        HOST_PHYSMEM_GB_OVERRIDE=48 report_host_memory_start >/dev/null 2>&1
    HOST_COMPRESSOR_GB_OVERRIDE=11.50 HOST_SWAP_USED_GB_OVERRIDE=2.00 \
        HOST_PHYSMEM_GB_OVERRIDE=48 \
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
    # The old text told the reader to raise LUCIDOS_E2E_COMPRESSOR_MAX_GB, which is
    # how a mis-calibrated guard teaches people to disable it.
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

test_the_old_ceiling_no_longer_stops_a_healthy_host
test_swap_over_the_limit_stops
test_swap_at_the_limit_does_not_stop
test_swap_wins_when_both_are_over
test_backstop_scales_with_ram
test_the_cap_bites_before_the_share_on_a_large_host
test_the_reading_that_stopped_the_nightly_now_runs_on
test_explicit_absolute_ceiling_overrides_the_share
test_percent_knob_is_honored
test_the_percent_knob_cannot_raise_the_backstop_past_the_cap
test_garbage_knobs_fall_back_to_defaults
test_garbage_swap_knob_does_not_stop_a_host_with_some_swap
test_negative_percent_cannot_stop_everything
test_unreadable_host_fails_open
test_unreadable_ram_drops_the_backstop_but_keeps_swap
test_swap_units_are_read_off_the_value
test_compressor_uses_the_reported_page_size
test_start_records_the_baseline_and_states_the_thresholds
test_the_swap_stop_calls_itself_measured_distress
test_the_compressor_stop_says_the_host_was_never_in_trouble
test_the_backstop_stop_does_not_clear_a_host_it_could_not_read
test_stop_report_is_silent_on_a_run_that_finished
test_stop_report_states_what_this_run_itself_cost
test_stop_report_does_not_advise_raising_the_ceiling
test_stop_exit_code_is_the_os_error_code

echo ""
echo "Passed: $PASS  Failed: $FAIL"
[ "$FAIL" -eq 0 ]
