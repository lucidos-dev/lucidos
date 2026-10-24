#!/bin/bash
# Tests for scripts/lib/e2e_warm.sh: background warm-up tasks on a GitHub e2e
# job, and the waits that later steps put in front of them.
#
# Hermetic: every task is a shell function or `true`/`false`/`sleep` in a
# throwaway directory. Nothing installs, pulls or signals anything.
#
# Run: ./scripts/lib/e2e_warm_test.sh
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SANDBOX="$(mktemp -d)"
trap 'rm -rf "$SANDBOX"' EXIT

PASS=0
FAIL=0
pass() { echo "  ok:   $*"; PASS=$((PASS + 1)); }
fail() { echo "  FAIL: $*"; FAIL=$((FAIL + 1)); }

# shellcheck source=scripts/lib/e2e_warm.sh
source "$SCRIPT_DIR/e2e_warm.sh"

fresh_dir() {
    export LUCIDOS_E2E_WARM_DIR="$SANDBOX/$1"
}

test_without_a_warm_dir_every_call_is_a_no_op() {
    echo "test: without LUCIDOS_E2E_WARM_DIR the wait returns at once and nothing starts"
    local rc=0
    unset LUCIDOS_E2E_WARM_DIR
    if e2e_warm_active; then fail "reads as active"; else pass "reads as inactive"; fi
    e2e_warm_wait npm >/dev/null 2>&1 || rc=$?
    if [ "$rc" -eq 0 ]; then pass "the wait is a no-op"; else fail "the wait returned $rc"; fi
}

test_a_finished_task_reports_and_passes() {
    echo "test: a task that succeeds is waited for, and the wait says so"
    local out rc=0
    fresh_dir ok
    greet() { echo "hello from a task"; }
    e2e_warm_start greet greet
    out="$(e2e_warm_wait greet 2>&1)" || rc=$?
    if [ "$rc" -eq 0 ]; then pass "the wait passes"; else fail "the wait returned $rc: $out"; fi
    if printf '%s' "$out" | grep -q "Warm-up: greet ready"; then pass "it names the task"; else fail "output: $out"; fi
    if grep -q "hello from a task" "$LUCIDOS_E2E_WARM_DIR/greet.log"; then pass "the task's output is in its log"; else fail "log missing"; fi
}

test_a_failed_task_fails_the_wait_with_its_log() {
    echo "test: a task that fails fails the wait and shows its log"
    local out rc=0
    fresh_dir failed
    broken() { echo "the mirror refused us"; return 3; }
    e2e_warm_start broken broken
    out="$(e2e_warm_wait broken 2>&1)" || rc=$?
    if [ "$rc" -eq "$E2E_WARM_FAILED_EXIT" ]; then pass "the wait fails with the setup-failure code, not the task's own"; else fail "the wait returned $rc"; fi
    if printf '%s' "$out" | grep -q "exit 3"; then pass "it gives the exit status"; else fail "output: $out"; fi
    if printf '%s' "$out" | grep -q "the mirror refused us"; then pass "it shows the log"; else fail "output: $out"; fi
}

test_no_names_waits_for_every_started_task() {
    echo "test: a bare wait covers every task that started, and a failure among them fails it"
    local out rc=0
    fresh_dir all
    e2e_warm_start first true
    e2e_warm_start second sleep 1
    out="$(e2e_warm_wait 2>&1)" || rc=$?
    if [ "$rc" -eq 0 ]; then pass "both finished"; else fail "returned $rc: $out"; fi
    if printf '%s' "$out" | grep -q "first ready" && printf '%s' "$out" | grep -q "second ready"; then
        pass "it waited for both"
    else
        fail "output: $out"
    fi
    e2e_warm_start third false
    rc=0
    e2e_warm_wait >/dev/null 2>&1 || rc=$?
    if [ "$rc" -ne 0 ]; then pass "a failed task among them fails the bare wait"; else fail "the bare wait passed"; fi
}

test_a_task_nobody_started_is_an_error() {
    echo "test: waiting for a task that never started fails, rather than passing silently"
    local out rc=0
    fresh_dir unstarted
    e2e_warm_start npm true
    out="$(e2e_warm_wait nmp 2>&1)" || rc=$?
    if [ "$rc" -eq "$E2E_WARM_FAILED_EXIT" ]; then pass "the wait fails with the setup-failure code"; else fail "the wait returned $rc"; fi
    if printf '%s' "$out" | grep -q "nmp was never started"; then pass "it names the task"; else fail "output: $out"; fi
}

test_a_task_that_outruns_the_limit_fails_the_wait() {
    echo "test: a task still running past the limit fails the wait"
    local out rc=0
    fresh_dir slow
    e2e_warm_start slow sleep 5
    out="$(E2E_WARM_TIMEOUT_SECS=1 e2e_warm_wait slow 2>&1)" || rc=$?
    if [ "$rc" -eq "$E2E_WARM_FAILED_EXIT" ]; then pass "the wait gives up with the setup-failure code"; else fail "the wait returned $rc"; fi
    if printf '%s' "$out" | grep -q "still running after 1s"; then pass "it says why"; else fail "output: $out"; fi
}

test_a_task_can_wait_for_another() {
    echo "test: a task that waits for another starts its work only after that one ends"
    local rc=0
    fresh_dir chain
    slow_install() { sleep 1; touch "$LUCIDOS_E2E_WARM_DIR/installed"; }
    after_install() { e2e_warm_wait slow_install >/dev/null && [ -f "$LUCIDOS_E2E_WARM_DIR/installed" ]; }
    e2e_warm_start slow_install slow_install
    e2e_warm_start after_install after_install
    e2e_warm_wait after_install >/dev/null 2>&1 || rc=$?
    if [ "$rc" -eq 0 ]; then pass "the second task saw the first one's work"; else fail "returned $rc"; fi
}

test_without_a_warm_dir_every_call_is_a_no_op
test_a_finished_task_reports_and_passes
test_a_failed_task_fails_the_wait_with_its_log
test_no_names_waits_for_every_started_task
test_a_task_nobody_started_is_an_error
test_a_task_that_outruns_the_limit_fails_the_wait
test_a_task_can_wait_for_another

echo ""
echo "e2e_warm: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
