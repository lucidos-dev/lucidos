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

# run_check <driver> [<sha>]: run main-check.sh; sets OUT and RC.
run_check() {
    OUT="$(MAIN_CHECK_E2E_DRIVER="$1" MAIN_CHECK_LOG_DIR="$SANDBOX/logs" "$MAIN_CHECK" "${2-$SHA}")"
    RC=$?
}

# verdict_field <name>: one field of the verdict line's JSON.
verdict_field() {
    printf '%s\n' "$OUT" | tail -1 | sed "s/^$MAIN_CHECK_VERDICT_PREFIX//" \
        | python3 -c 'import json, sys; v = json.load(sys.stdin).get(sys.argv[1]); print("" if v is None else v)' "$1"
}

GREEN_REPORT="[e2e-github] run x: remote engine api
$MAIN_CHECK_WATCHING_MARK$RUN_URL
══ engine (GitHub) ══
test result: ok. 4000 passed
── engine exit code: 0 ──
══ api (GitHub) ══
── api exit code: 0 ──"

RED_REPORT="$MAIN_CHECK_WATCHING_MARK$RUN_URL
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

test_a_green_run_passes_and_names_its_run() {
    echo "test: a green run prints its run line, then a pass verdict, and exits 0"
    run_check "$(fake_driver green 0 "$GREEN_REPORT")"
    if [ "$RC" = 0 ]; then pass "exit 0"; else fail "exit $RC"; fi
    if [ "$(printf '%s\n' "$OUT" | head -1)" = "$MAIN_CHECK_RUN_PREFIX$RUN_URL" ]; then
        pass "the run line comes first"
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

test_a_crash_with_no_report_still_fails_with_a_tail() {
    echo "test: a red exit with no report still fails, with the last lines as its tail"
    run_check "$(fake_driver crash 2 "$MAIN_CHECK_WATCHING_MARK$RUN_URL
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
echo "$MAIN_CHECK_WATCHING_MARK$RUN_URL"
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

test_a_green_run_passes_and_names_its_run
test_a_red_run_names_what_failed
test_no_verdict_is_its_own_answer
test_a_crash_with_no_report_still_fails_with_a_tail
test_the_verdict_maps_every_exit
test_shutdown_leaves_the_driver_time_to_cancel_its_run

echo ""
echo "Passed: $PASS  Failed: $FAIL"
[ "$FAIL" -eq 0 ]
