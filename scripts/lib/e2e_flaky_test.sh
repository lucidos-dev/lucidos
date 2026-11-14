#!/bin/bash
# Tests for the engine suite's one rerun: scripts/lib/e2e_flaky.sh.
#
# Hermetic. The engine test script is a stub that records its arguments, so
# nothing here compiles or runs a test.
#
# Run: ./scripts/lib/e2e_flaky_test.sh
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SANDBOX="$(mktemp -d)"
trap 'rm -rf "$SANDBOX"' EXIT

PASS=0
FAIL=0
pass() { echo "  ok:   $*"; PASS=$((PASS + 1)); }
fail() { echo "  FAIL: $*"; FAIL=$((FAIL + 1)); }

# shellcheck source=scripts/lib/e2e_flaky.sh
source "$SCRIPT_DIR/e2e_flaky.sh"

# A real engine failure on a runner: one test, one panic.
FIXTURE="$SCRIPT_DIR/main_check_fixtures/engine-failure.txt"
FAILED_TEST="engine::git_ops::answer_tests::unmeetable_timeout_errors_and_classifies_unknown"

# stub_runner <name> <exit> <summary line>: a stand-in for test-engine.sh.
stub_runner() {
    local path="$SANDBOX/$1"
    {
        echo '#!/bin/bash'
        echo "echo \"\$*\" >\"$SANDBOX/$1.args\""
        echo "echo '$3'"
        echo "exit $2"
    } >"$path"
    chmod +x "$path"
    printf '%s\n' "$path"
}

# first_run_log <name>: a copy of the fixture, as the shard's log.
first_run_log() {
    cp "$FIXTURE" "$SANDBOX/$1.log"
    printf '%s\n' "$SANDBOX/$1.log"
}

test_a_cargo_failure_names_the_test_to_rerun() {
    echo "test: the failed test is read from a real cargo failure"
    if [ "$(e2e_failed_engine_tests "$FIXTURE")" = "$FAILED_TEST" ]; then
        pass "one name, exact"
    else
        fail "names: $(e2e_failed_engine_tests "$FIXTURE")"
    fi
}

test_a_failure_that_does_not_add_up_is_never_rerun() {
    echo "test: a count that disagrees with the list, or no summary at all, reruns nothing"
    local log="$SANDBOX/odd.log"
    sed 's/ 1 failed;/ 2 failed;/' "$FIXTURE" >"$log"
    if [ -z "$(e2e_failed_engine_tests "$log")" ]; then pass "a list shorter than the count"; else fail "listed: $(e2e_failed_engine_tests "$log")"; fi
    printf 'error[E0432]: unresolved import\nerror: could not compile "lucidos-engine"\n' >"$log"
    if [ -z "$(e2e_failed_engine_tests "$log")" ]; then pass "a compile error"; else fail "listed: $(e2e_failed_engine_tests "$log")"; fi
    sed '/^test result:/d' "$FIXTURE" >"$log"
    if [ -z "$(e2e_failed_engine_tests "$log")" ]; then pass "a run that ended with no summary"; else fail "listed: $(e2e_failed_engine_tests "$log")"; fi
}

test_a_test_that_passes_its_rerun_is_flaky() {
    echo "test: a passing rerun succeeds and ends the log with the flaky block"
    local log runner
    log="$(first_run_log passes)"
    runner="$(stub_runner passes-run 0 'test result: ok. 1 passed; 0 failed; 0 ignored')"
    if e2e_rerun_failed_engine_tests "$log" "$runner" >/dev/null; then pass "the rerun counts as a pass"; else fail "rerun failed"; fi
    if [ "$(cat "$SANDBOX/passes-run.args")" = "-- -- $FAILED_TEST --exact" ]; then
        pass "it ran the one test, by exact name"
    else
        fail "runner args: $(cat "$SANDBOX/passes-run.args")"
    fi
    if [ "$(tail -3 "$log")" = "$E2E_FLAKY_ENGINE_MARK
$FAILED_TEST
$E2E_FLAKY_END_MARK" ]; then
        pass "the log ends with the flaky block"
    else
        fail "log tail: $(tail -3 "$log")"
    fi
}

test_a_test_that_fails_its_rerun_stays_red() {
    echo "test: a failing rerun fails, and leaves no flaky block"
    local log runner
    log="$(first_run_log fails)"
    runner="$(stub_runner fails-run 101 'test result: FAILED. 0 passed; 1 failed')"
    if e2e_rerun_failed_engine_tests "$log" "$runner" >/dev/null; then fail "a failing rerun passed"; else pass "the rerun fails"; fi
    if grep -q "$E2E_FLAKY_ENGINE_MARK" "$log"; then fail "a flaky block was written"; else pass "no flaky block"; fi
}

test_a_rerun_that_ran_nothing_is_not_a_pass() {
    echo "test: a rerun that matched no test fails"
    local log runner
    log="$(first_run_log nothing)"
    runner="$(stub_runner nothing-run 0 'test result: ok. 0 passed; 0 failed; 0 ignored')"
    if e2e_rerun_failed_engine_tests "$log" "$runner" >/dev/null; then fail "an empty rerun passed"; else pass "the empty rerun fails"; fi
}

test_nothing_to_rerun_runs_nothing() {
    echo "test: a log with no parsed failure never calls the runner"
    local log="$SANDBOX/compile.log" runner
    echo 'error: could not compile "lucidos-engine"' >"$log"
    runner="$(stub_runner never-run 0 'test result: ok. 1 passed;')"
    if e2e_rerun_failed_engine_tests "$log" "$runner" >/dev/null; then fail "rerun succeeded"; else pass "no rerun"; fi
    if [ -e "$SANDBOX/never-run.args" ]; then fail "the runner was called"; else pass "the runner was not called"; fi
}

test_a_cargo_failure_names_the_test_to_rerun
test_a_failure_that_does_not_add_up_is_never_rerun
test_a_test_that_passes_its_rerun_is_flaky
test_a_test_that_fails_its_rerun_stays_red
test_a_rerun_that_ran_nothing_is_not_a_pass
test_nothing_to_rerun_runs_nothing

echo ""
echo "Passed: $PASS  Failed: $FAIL"
[ "$FAIL" -eq 0 ]
