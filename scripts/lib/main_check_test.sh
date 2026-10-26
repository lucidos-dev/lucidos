#!/bin/bash
# Tests for the main check's script half: scripts/main-check.sh and
# scripts/lib/main_check.sh (ADR 0464).
#
# Hermetic. Each case swaps the e2e driver for a fake through
# MAIN_CHECK_E2E_DRIVER and writes its log under a sandbox, so nothing here
# pushes, calls gh or reaches the network.
#
# Run: ./scripts/lib/main_check_test.sh
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
MAIN_CHECK="$SCRIPT_DIR/../main-check.sh"
SANDBOX="$(mktemp -d)"
trap 'rm -rf "$SANDBOX"' EXIT

PASS=0
FAIL=0
pass() { echo "  ok:   $*"; PASS=$((PASS + 1)); }
fail() { echo "  FAIL: $*"; FAIL=$((FAIL + 1)); }

# shellcheck source=scripts/lib/e2e_github.sh
source "$SCRIPT_DIR/e2e_github.sh"
# shellcheck source=scripts/lib/main_check.sh
source "$SCRIPT_DIR/main_check.sh"

RUN_URL="https://github.com/lucidos-dev/lucidos/actions/runs/4242"
RUN_BRANCH="e2e/20261019-120000-ab12"
SHA="0123456789abcdef0123456789abcdef01234567"

# fake_driver <name> <exit> <body>: a stand-in for e2e-github.sh that records
# its arguments, prints <body>, and exits <exit>.
fake_driver() {
    local path="$SANDBOX/$1"
    {
        echo '#!/bin/bash'
        echo "echo \"\$*\" >\"$SANDBOX/$1.args\""
        echo "cat <<'BODY'"
        printf '%s\n' "$3"
        echo "BODY"
        echo "exit $2"
    } >"$path"
    chmod +x "$path"
    printf '%s\n' "$path"
}

# run_check <driver> [<sha> [<more args>...]]: run main-check.sh; sets OUT and RC.
run_check() {
    local driver="$1"
    shift
    [ $# -gt 0 ] || set -- "$SHA"
    OUT="$(MAIN_CHECK_E2E_DRIVER="$driver" MAIN_CHECK_LOG_DIR="$SANDBOX/logs" "$MAIN_CHECK" "$@")"
    RC=$?
}

# verdict_field <name>: one field of the verdict line's JSON.
verdict_field() {
    printf '%s\n' "$OUT" | tail -1 | sed "s/^$MAIN_CHECK_VERDICT_PREFIX//" \
        | python3 -c 'import json, sys; v = json.load(sys.stdin).get(sys.argv[1]); print("" if v is None else v)' "$1"
}

GREEN_REPORT="[e2e-github] run x: remote engine api
$E2E_GITHUB_LINKED_MARK$RUN_URL $RUN_BRANCH
══ engine (GitHub) ══
test result: ok. 4000 passed
── engine exit code: 0 ──
══ api (GitHub) ══
── api exit code: 0 ──"

RED_REPORT="$E2E_GITHUB_LINKED_MARK$RUN_URL $RUN_BRANCH
══ engine (GitHub) ══
running 4000 tests
failures:
    engine::main_check::tests::a_red_check_blocks
test result: FAILED. 3999 passed; 1 failed
── engine exit code: 101 ──
══ api (GitHub) ══
test result: ok. 12 passed
── api exit code: 0 ──
══ browser: chromium (GitHub) ══
  1 failed
    [chromium] chat.spec.ts:12 sends a message
── chromium total: 3 tests over 1 invocation(s) ──

── Per-project exit codes ──
  chromium: 1
  mobile: 0"

FIXTURES="$SCRIPT_DIR/main_check_fixtures"

# fixture_log <section title> <fixture> <exit-code line>...: a driver log with one
# failed phase around a fixture cut from a real failing run.
fixture_log() {
    local title="$1" fixture="$2" log="$SANDBOX/fixture.log"
    {
        echo "$E2E_GITHUB_LINKED_MARK$RUN_URL $RUN_BRANCH"
        echo "══ $title (GitHub) ══"
        cat "$FIXTURES/$fixture"
        shift 2
        printf '%s\n' "$@"
    } >"$log"
    printf '%s\n' "$log"
}

# failures_of <log>: the verdict's failures list, as JSON.
failures_of() {
    main_check_verdict_line fail 1 "$1" | sed "s/^$MAIN_CHECK_VERDICT_PREFIX//" \
        | python3 -c 'import json, sys; print(json.dumps(json.load(sys.stdin)["failures"]))'
}

# check_failures <log> <python assertion over `f`, the failures list> <message>
check_failures() {
    if failures_of "$1" | python3 -c 'import json, sys; f = json.load(sys.stdin); sys.exit(0 if eval(sys.argv[1]) else 1)' "$2"; then
        pass "$3"
    else
        fail "$3: $(failures_of "$1")"
    fi
}

test_a_cargo_failure_names_its_test_and_panic() {
    echo "test: a failed cargo test is named, with its panic"
    local log
    log="$(fixture_log engine engine-failure.txt "── engine exit code: 101 ──")"
    check_failures "$log" 'len(f) == 1 and f[0]["suite"] == "engine" and f[0]["name"] == "engine::git_ops::answer_tests::unmeetable_timeout_errors_and_classifies_unknown"' \
        "one failure, in the engine suite, by its full name"
    check_failures "$log" '"a 1ns ceiling cannot be met" in f[0]["detail"] and "panicked at" in f[0]["detail"] and "test result" not in f[0]["detail"]' \
        "the detail holds the panic and stops at the next section"
}

test_a_playwright_failure_names_its_spec_and_last_attempt() {
    echo "test: a failed Playwright test is named, with its last attempt's error"
    local log
    log="$(fixture_log "browser: chromium" chromium-failure.txt "── chromium total: 111 tests over 1 invocation(s) ──" "" "── Per-project exit codes ──" "  chromium: 1")"
    check_failures "$log" 'len(f) == 1 and f[0]["suite"] == "browser chromium" and f[0]["name"].startswith("e2e/dashboard-desktop.spec.ts:256:3 › dashboard on desktop › the Home icon")' \
        "one failure, in the chromium phase, without the project prefix"
    check_failures "$log" '"Timeout 15000ms exceeded" in f[0]["detail"] and ".home-thread-btn" in f[0]["detail"] and "attachment" not in f[0]["detail"] and "Retry" not in f[0]["detail"]' \
        "the detail is the error and its code frame, without the attachments"
}

test_a_flaky_playwright_test_is_not_a_failure() {
    echo "test: a test Playwright lists as flaky is not listed as failed"
    local log="$SANDBOX/flaky.log"
    {
        echo "══ browser: chromium (GitHub) ══"
        echo ""
        echo "  1) [chromium] › e2e/a.spec.ts:3:1 › flaky one "
        echo ""
        echo "    Error: first try"
        echo ""
        echo "  2) [chromium] › e2e/b.spec.ts:4:1 › broken one "
        echo ""
        echo "    Error: always"
        echo ""
        echo "  1 failed"
        echo "    [chromium] › e2e/b.spec.ts:4:1 › broken one "
        echo "  1 flaky"
        echo "    [chromium] › e2e/a.spec.ts:3:1 › flaky one "
        echo "  8 passed (1.0m)"
        echo "── Per-project exit codes ──"
        echo "  chromium: 1"
    } >"$log"
    check_failures "$log" '[t["name"] for t in f] == ["e2e/b.spec.ts:4:1 › broken one"] and f[0]["detail"] == "Error: always"' \
        "only the test that failed twice is named"
}

test_many_failures_keep_every_name_inside_the_budget() {
    echo "test: 300 failures keep every name, and their details share one budget"
    local log="$SANDBOX/many.log" i
    {
        echo "══ engine (GitHub) ══"
        echo "failures:"
        echo ""
        for i in $(seq 1 300); do
            echo "---- engine::t::case_$i stdout ----"
            echo "thread 'x' panicked at src/lib.rs:1:1:"
            printf 'detail %s %s\n' "$i" "$(printf 'x%.0s' $(seq 1 400))"
            echo ""
        done
        echo "failures:"
        for i in $(seq 1 300); do echo "    engine::t::case_$i"; done
        echo ""
        echo "test result: FAILED. 0 passed; 300 failed"
        echo "── engine exit code: 101 ──"
    } >"$log"
    check_failures "$log" 'len(f) == 300 and all(t["name"] == "engine::t::case_%d" % (i + 1) for i, t in enumerate(f))' \
        "all 300 names, in order"
    check_failures "$log" 'sum(len(t["detail"]) for t in f) <= 8000 and all(t["detail"] for t in f)' \
        "the details together stay within the budget"
}

# flaky_of <verdict> <log>: the verdict's flaky list, as JSON.
flaky_of() {
    main_check_verdict_line "$1" 0 "$2" | sed "s/^$MAIN_CHECK_VERDICT_PREFIX//" \
        | python3 -c 'import json, sys; print(json.dumps(json.load(sys.stdin)["flaky"]))'
}

check_flaky() {
    if flaky_of "$1" "$2" | python3 -c 'import json, sys; f = json.load(sys.stdin); sys.exit(0 if eval(sys.argv[1]) else 1)' "$3"; then
        pass "$4"
    else
        fail "$4: $(flaky_of "$1" "$2")"
    fi
}

ENGINE_TEST="engine::git_ops::answer_tests::unmeetable_timeout_errors_and_classifies_unknown"

# engine_rerun_log <passed|failed>: the first run's real failure, then a rerun
# that passed (with the flaky block) or failed the same test again.
engine_rerun_log() {
    local log="$SANDBOX/rerun-$1.log"
    {
        echo "══ engine (GitHub) ══"
        cat "$FIXTURES/engine-failure.txt"
        echo ""
        echo "── rerunning 1 failed engine test(s), once ──"
        if [ "$1" = passed ]; then
            echo "test result: ok. 1 passed; 0 failed; 0 ignored"
            echo "$E2E_FLAKY_ENGINE_MARK"
            echo "$ENGINE_TEST"
            echo "$E2E_FLAKY_END_MARK"
            echo "── engine exit code: 0 ──"
        else
            sed -e 's/panicked at .*/panicked at the rerun:1:1:/' "$FIXTURES/engine-failure.txt" | sed -n '/^failures:/,$p'
            echo "── engine exit code: 101 ──"
        fi
    } >"$log"
    printf '%s\n' "$log"
}

test_an_engine_test_that_passed_its_rerun_is_flaky_with_its_first_error() {
    echo "test: a rerun that passed lists the test as flaky, with the first run's panic"
    local log
    log="$(engine_rerun_log passed)"
    check_flaky pass "$log" 'len(f) == 1 and f[0]["suite"] == "engine" and f[0]["name"] == "'"$ENGINE_TEST"'"' \
        "one flaky test, in the engine suite"
    check_flaky pass "$log" '"a 1ns ceiling cannot be met" in f[0]["detail"]' \
        "its detail is the first run's panic"
    check_failures "$log" 'f == []' "a pass names no failure"
}

test_an_engine_test_that_failed_its_rerun_is_a_failure() {
    echo "test: a rerun that failed names the failure from the rerun"
    local log
    log="$(engine_rerun_log failed)"
    check_failures "$log" 'len(f) == 1 and f[0]["name"] == "'"$ENGINE_TEST"'" and "the rerun" in f[0]["detail"]' \
        "the failure carries the rerun's panic"
    check_flaky fail "$log" 'f == []' "and nothing is flaky"
}

test_playwright_flaky_tests_carry_their_first_attempt() {
    echo "test: tests Playwright lists as flaky carry the error of their first attempt"
    local log="$SANDBOX/pw-flaky.log"
    {
        echo "══ browser: chromium (GitHub) ══"
        echo ""
        echo "  1) [chromium] › e2e/a.spec.ts:3:1 › flaky one "
        echo ""
        echo "    Error: first try"
        echo ""
        echo "    Retry #1 ───────────────────────────────────────────────────────────────────────────────────────"
        echo ""
        echo "  1 flaky"
        echo "    [chromium] › e2e/a.spec.ts:3:1 › flaky one "
        echo "  8 passed (1.0m)"
        echo "── Per-project exit codes ──"
        echo "  chromium: 0"
    } >"$log"
    check_flaky pass "$log" 'len(f) == 1 and f[0]["suite"] == "browser chromium" and f[0]["name"] == "e2e/a.spec.ts:3:1 › flaky one" and f[0]["detail"] == "Error: first try"' \
        "named, with the first attempt's error"
}

test_flaky_tests_never_hide_a_real_failure() {
    echo "test: a flaky engine test beside a browser test that failed twice is a failure"
    local log="$SANDBOX/both.log"
    {
        sed -n '/^══ engine/,/^── engine exit code/p' "$(engine_rerun_log passed)"
        echo "══ browser: chromium (GitHub) ══"
        cat "$FIXTURES/chromium-failure.txt"
        echo "── Per-project exit codes ──"
        echo "  chromium: 1"
    } >"$log"
    check_failures "$log" '[t["suite"] for t in f] == ["browser chromium"]' "the browser failure is listed"
    check_flaky fail "$log" '[t["suite"] for t in f] == ["engine"]' "the engine test is still reported flaky"
}

test_nothing_parsed_leaves_the_tail_to_speak() {
    echo "test: output with no recognised failure lists none, and keeps its tail"
    run_check "$(fake_driver crash2 1 "$E2E_GITHUB_LINKED_MARK$RUN_URL $RUN_BRANCH
══ wasm (GitHub) ══
Error: something odd
── wasm exit code: 1 ──")"
    if [ "$(verdict_field failures)" = "[]" ] && [[ "$(verdict_field tail)" == *"something odd"* ]]; then
        pass "failures empty, tail holds the output"
    else
        fail "verdict line: $(printf '%s\n' "$OUT" | tail -1)"
    fi
    run_check "$(fake_driver green2 0 "$GREEN_REPORT")"
    if [ "$(verdict_field failures)" = "[]" ]; then pass "a pass lists no failures"; else fail "pass: $(verdict_field failures)"; fi
}

test_a_green_run_passes_and_names_its_run() {
    echo "test: a green run prints its run line, then a pass verdict, and exits 0"
    run_check "$(fake_driver green 0 "$GREEN_REPORT")"
    if [ "$RC" = 0 ]; then pass "exit 0"; else fail "exit $RC"; fi
    if [ "$(printf '%s\n' "$OUT" | head -1)" = "$MAIN_CHECK_RUN_PREFIX$RUN_URL $RUN_BRANCH" ]; then
        pass "the run line comes first, naming the run and its branch"
    else
        fail "first line: $(printf '%s\n' "$OUT" | head -1)"
    fi
    if [ "$(printf '%s\n' "$OUT" | grep -c .)" = 2 ]; then
        pass "stdout holds only the protocol"
    else
        fail "stdout was: $OUT"
    fi
    if [ "$(verdict_field verdict)" = pass ] && [ "$(verdict_field run_url)" = "$RUN_URL" ] && [ -z "$(verdict_field tail)" ]; then
        pass "the verdict names pass and the run, with no tail"
    else
        fail "verdict line: $(printf '%s\n' "$OUT" | tail -1)"
    fi
    if [ "$(cat "$SANDBOX/green.args")" = "main-check --commit $SHA" ]; then
        pass "the driver ran the main-check selection on the named commit"
    else
        fail "driver args: $(cat "$SANDBOX/green.args")"
    fi
    if grep -q "4000 passed" "$(verdict_field log)"; then pass "the full output is in the log"; else fail "log missing output"; fi
}

test_a_red_run_names_what_failed() {
    echo "test: a red run summarises the failing phases and carries their tail"
    run_check "$(fake_driver red 1 "$RED_REPORT")"
    if [ "$RC" = 1 ]; then pass "exit 1"; else fail "exit $RC"; fi
    if [ "$(verdict_field verdict)" = fail ]; then pass "verdict fail"; else fail "verdict $(verdict_field verdict)"; fi
    if [ "$(verdict_field summary)" = "Failed: engine, browser chromium." ]; then
        pass "the summary names each failing phase"
    else
        fail "summary: $(verdict_field summary)"
    fi
    local tail
    tail="$(verdict_field tail)"
    if [[ "$tail" == *"a_red_check_blocks"* ]] && [[ "$tail" == *"chat.spec.ts:12"* ]] && [[ "$tail" != *"12 passed"* ]]; then
        pass "the tail holds the failing phases only"
    else
        fail "tail: $tail"
    fi
}

test_no_verdict_is_its_own_answer() {
    echo "test: a run that never reached a verdict says why, and exits 76"
    run_check "$(fake_driver nothing 76 "ERROR: GitHub mode cannot run: there is no 'lucidos' remote.
       Fix that, or run the suite with --local.")"
    if [ "$RC" = "$E2E_GITHUB_NO_VERDICT_EXIT" ]; then pass "exit 76"; else fail "exit $RC"; fi
    if [ "$(verdict_field verdict)" = no-verdict ] && [ -z "$(verdict_field run_url)" ]; then
        pass "verdict no-verdict, with no run"
    else
        fail "verdict line: $(printf '%s\n' "$OUT" | tail -1)"
    fi
    if [ "$(verdict_field summary)" = "GitHub mode cannot run: there is no 'lucidos' remote." ]; then
        pass "the summary is the driver's own reason"
    else
        fail "summary: $(verdict_field summary)"
    fi
    run_check "$(fake_driver unused 0 "")" ""
    if [ "$RC" = "$E2E_GITHUB_NO_VERDICT_EXIT" ] && [ "$(verdict_field verdict)" = no-verdict ] && [ ! -e "$SANDBOX/unused.args" ]; then
        pass "no sha runs nothing and is no verdict"
    else
        fail "no sha: rc=$RC out=$OUT"
    fi
}

test_a_shard_whose_setup_failed_is_no_verdict_and_names_the_task() {
    echo "test: a report whose only bad shard failed its setup is no verdict, naming the shard and the task"
    run_check "$(fake_driver setup-failed 76 "$E2E_GITHUB_LINKED_MARK$RUN_URL $RUN_BRANCH
══ browser: chromium (GitHub) ══
ERROR: warm-up task apt failed (exit 1). Its log ends:
ERROR: the chromium-5-7 shard's apt setup failed (exit 77), so it tested nothing.

── Per-project exit codes ──
  chromium: 76")"
    if [ "$RC" = "$E2E_GITHUB_NO_VERDICT_EXIT" ] && [ "$(verdict_field verdict)" = no-verdict ]; then
        pass "no verdict, so no red and no fix thread"
    else
        fail "rc=$RC out=$OUT"
    fi
    if [ "$(verdict_field summary)" = "the chromium-5-7 shard's apt setup failed (exit 77), so it tested nothing." ]; then
        pass "the summary names the shard and the warm-up task"
    else
        fail "summary: $(verdict_field summary)"
    fi
}

test_a_crash_with_no_report_still_fails_with_a_tail() {
    echo "test: a red exit with no report still fails, with the last lines as its tail"
    run_check "$(fake_driver crash 2 "$E2E_GITHUB_LINKED_MARK$RUN_URL $RUN_BRANCH
something broke here")"
    if [ "$(verdict_field verdict)" = fail ] && [ "$(verdict_field summary)" = "The run failed (exit 2)." ] \
        && [[ "$(verdict_field tail)" == *"something broke here"* ]]; then
        pass "fail, with the exit code and the last lines"
    else
        fail "verdict line: $(printf '%s\n' "$OUT" | tail -1)"
    fi
}

test_the_verdict_maps_every_exit() {
    echo "test: the driver's exit maps to one verdict and back to one exit"
    local wrong="" rc
    for rc in 0 1 2 101 "$E2E_GITHUB_NO_VERDICT_EXIT"; do
        case "$(main_check_verdict "$rc"):$(main_check_exit "$(main_check_verdict "$rc")")" in
            pass:0) [ "$rc" = 0 ] || wrong="$wrong $rc" ;;
            no-verdict:76) [ "$rc" = 76 ] || wrong="$wrong $rc" ;;
            fail:1) [ "$rc" != 0 ] && [ "$rc" != 76 ] || wrong="$wrong $rc" ;;
            *) wrong="$wrong $rc" ;;
        esac
    done
    if [ -z "$wrong" ]; then pass "0 pass, 76 no verdict, anything else fail"; else fail "misread:$wrong"; fi
}

# Engine shutdown sends SIGTERM to main-check.sh's whole process group. The
# driver's cleanup then writes to stderr, the pipe main_check_follow reads,
# before it cancels the run. A reader that died with the group took the driver
# down with SIGPIPE, and the run went on.
test_shutdown_leaves_the_driver_time_to_cancel_its_run() {
    echo "test: SIGTERM to the group lets the driver's cleanup finish before main-check.sh exits"
    local driver="$SANDBOX/stopped-driver" out="$SANDBOX/stopped.out" pid waited=0
    cat >"$driver" <<DRIVER
#!/bin/bash
cleanup() {
    echo "[e2e-github] cancelling run 4242" >&2
    echo "[e2e-github] deleting the branch" >&2
    echo cancelled >"$SANDBOX/stopped.cancelled"
}
trap cleanup EXIT
trap 'exit 130' INT TERM
echo "$E2E_GITHUB_LINKED_MARK$RUN_URL $RUN_BRANCH"
touch "$SANDBOX/stopped.started"
while :; do sleep 0.1; done
DRIVER
    chmod +x "$driver"
    set -m
    MAIN_CHECK_E2E_DRIVER="$driver" MAIN_CHECK_LOG_DIR="$SANDBOX/logs" "$MAIN_CHECK" "$SHA" >"$out" &
    pid=$!
    set +m
    while [ ! -e "$SANDBOX/stopped.started" ] && [ "$waited" -lt 100 ]; do
        sleep 0.1
        waited=$((waited + 1))
    done
    kill -TERM -- "-$pid"
    wait "$pid"
    if [ -e "$SANDBOX/stopped.cancelled" ]; then
        pass "the driver's cleanup ran to its end"
    else
        fail "the cleanup died before it cancelled the run"
    fi
    if grep -q "deleting the branch" "$SANDBOX"/logs/*; then
        pass "its output reached the log"
    else
        fail "the log lost the cleanup's output"
    fi
    OUT="$(cat "$out")"
    if [ "$(verdict_field verdict)" = fail ]; then
        pass "main-check.sh still printed its verdict line, after the driver ended"
    else
        fail "stdout was: $OUT"
    fi
}

test_attach_hands_the_run_to_the_driver() {
    echo "test: --attach hands the run and its branch to the driver"
    run_check "$(fake_driver attach 0 "$GREEN_REPORT")" "$SHA" --attach "$RUN_URL" "$RUN_BRANCH"
    if [ "$(cat "$SANDBOX/attach.args")" = "main-check --commit $SHA --attach $RUN_URL --branch $RUN_BRANCH" ]; then
        pass "the driver picks up the named run"
    else
        fail "driver args: $(cat "$SANDBOX/attach.args")"
    fi
    if [ "$RC" = 0 ] && [ "$(verdict_field verdict)" = pass ]; then
        pass "its verdict is the check's"
    else
        fail "rc=$RC: $OUT"
    fi
    run_check "$(fake_driver attach-bad 0 "$GREEN_REPORT")" "$SHA" --attach "$RUN_URL"
    if [ "$RC" = "$E2E_GITHUB_NO_VERDICT_EXIT" ] && [ ! -e "$SANDBOX/attach-bad.args" ]; then
        pass "an attach without its branch is a usage error, and runs nothing"
    else
        fail "rc=$RC: $OUT"
    fi
}

# Engine shutdown sends SIGUSR2 to the group once the run is on record. The
# driver detaches and leaves the run going, and main-check.sh still reports.
test_a_detach_lets_the_driver_leave_its_run() {
    echo "test: SIGUSR2 to the group lets the driver detach before main-check.sh exits"
    local driver="$SANDBOX/hup-driver" out="$SANDBOX/hup.out" pid waited=0
    cat >"$driver" <<DRIVER
#!/bin/bash
trap 'echo detached >"$SANDBOX/hup.stopped"; exit 140' USR2
trap 'echo cancelled >"$SANDBOX/hup.stopped"; exit 130' TERM
echo "$E2E_GITHUB_LINKED_MARK$RUN_URL $RUN_BRANCH"
touch "$SANDBOX/hup.started"
while :; do sleep 0.1; done
DRIVER
    chmod +x "$driver"
    set -m
    MAIN_CHECK_E2E_DRIVER="$driver" MAIN_CHECK_LOG_DIR="$SANDBOX/logs" "$MAIN_CHECK" "$SHA" >"$out" &
    pid=$!
    set +m
    while [ ! -e "$SANDBOX/hup.started" ] && [ "$waited" -lt 100 ]; do
        sleep 0.1
        waited=$((waited + 1))
    done
    kill -USR2 -- "-$pid"
    wait "$pid"
    if [ "$(cat "$SANDBOX/hup.stopped" 2>/dev/null)" = detached ]; then
        pass "the driver heard the detach, not a cancel"
    else
        fail "the driver recorded: $(cat "$SANDBOX/hup.stopped" 2>/dev/null)"
    fi
    OUT="$(cat "$out")"
    if [ "$(printf '%s\n' "$OUT" | head -1)" = "$MAIN_CHECK_RUN_PREFIX$RUN_URL $RUN_BRANCH" ] \
        && [ -n "$(verdict_field verdict)" ]; then
        pass "the run line went out, and main-check.sh still printed its verdict line"
    else
        fail "stdout was: $OUT"
    fi
}

test_a_green_run_passes_and_names_its_run
test_attach_hands_the_run_to_the_driver
test_a_red_run_names_what_failed
test_no_verdict_is_its_own_answer
test_a_shard_whose_setup_failed_is_no_verdict_and_names_the_task
test_a_crash_with_no_report_still_fails_with_a_tail
test_the_verdict_maps_every_exit
test_a_cargo_failure_names_its_test_and_panic
test_a_playwright_failure_names_its_spec_and_last_attempt
test_a_flaky_playwright_test_is_not_a_failure
test_many_failures_keep_every_name_inside_the_budget
test_an_engine_test_that_passed_its_rerun_is_flaky_with_its_first_error
test_an_engine_test_that_failed_its_rerun_is_a_failure
test_playwright_flaky_tests_carry_their_first_attempt
test_flaky_tests_never_hide_a_real_failure
test_nothing_parsed_leaves_the_tail_to_speak
test_shutdown_leaves_the_driver_time_to_cancel_its_run
test_a_detach_lets_the_driver_leave_its_run

echo ""
echo "Passed: $PASS  Failed: $FAIL"
[ "$FAIL" -eq 0 ]
