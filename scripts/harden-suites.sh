#!/usr/bin/env bash
#
# harden-suites.sh: run the `/harden` Phase 4.5 test suites, early or late,
# and say whether their result still describes the branch (ADR 0292).
#
#   ./scripts/harden-suites.sh start --early   # at the Phase 1 kickoff
#   ./scripts/harden-suites.sh stop            # before a fix the result cannot survive
#   ./scripts/harden-suites.sh wait            # at Phase 4.5: join, then the verdict
#                   [--review-abandoned]       #   Phase 3 gave up on the other-model review
#                   [--budget <seconds>]       #   how long to wait (default 230)
#   ./scripts/harden-suites.sh verdict         # the verdict alone
#   ./scripts/harden-suites.sh start           # a normal run, when the verdict says RERUN
#   ./scripts/harden-suites.sh timings         # each suite's start, end and exit, as JSON
#
# `start` selects suites from `git diff main...HEAD`, runs them, and records
# the start commit. An early run skips the two Codex driver modules. Once the
# other-model review is joined, `wait` starts them alone in the background
# and joins them like any suite, so no `wait` blocks past its budget.
#
# The touched-spec selection is slow on a loaded host, and only the
# `e2e-touched` suite reads it. So `start` launches every other suite first,
# selects the specs while they run, and launches whatever the specs add. A
# failed selection stops the suites it launched and fails the start. The
# `suites` file is written last, so a run still selecting is never a verdict.
#
# `start` also records the tier. An integrated sub-thread's branch runs the
# fast tier, which the engine names (`lucidos hardened expected-tier`); every
# other branch runs the full one. The engine's answer wins: HARDEN_TIER names
# the tier only when no engine answers. HS_LUCIDOS_CMD replaces the CLI in
# the tests, which must never ask a real engine.
#
# A FAIL verdict records what failed in .lucidos/harden-retry: engine tests and
# browser specs by name, any other failed suite whole. The next `start` is then
# a retry round, which runs only those. A retry round never passes the harden:
# once its tests pass it clears the record and says RERUN, so the full set runs
# once more (ADR 0536).
#
# The touched specs mobile-webkit runs go to GitHub in their own suite,
# e2e-touched-webkit, as the run starts. The rest run on this host.
#
# The verdict prints one line per suite: PASS, SKIP <reason>, FAIL, VOID
# <reason>, RUNNING or MISSING. A touched-spec suite refused by a busy e2e lock
# is a SKIP, and a SKIP passes (ADR 0541). An early result stays valid only
# while every path changed since the start commit is on HARDEN_SAFE_PATHS
# (scripts/lib/harden_suites.sh). Each
# suite also stamps the changed paths when it exits, so an edit made while it
# ran voids it even if the edit was later undone.
#
# A cargo suite waits on the build memory gate before it starts, and exits 72
# when the host never recovers (ADR 0351).
#
# The run carries the caller's agent run marker, so the turn-end teardown
# still reaches it (ADR 0391). A Switch leaves it running for the resumed
# session (ADR 0478). `stop` walks the pid tree from the recorded pid, never
# by name.
#
# A VOID on an allowlisted path names the files that turned the allowlist
# off. Add each to HS_READ_CHECK_KNOWN if it never reads the path.
#
# State and logs: .lucidos/harden-suites/ in the worktree (gitignored).
#
# Exit status: 0 every suite PASS or SKIP, 1 a FAIL or bad usage, 2 RERUN needed
# (a VOID or MISSING line, and no FAIL), 3 still running.

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/lib/harden_suites.sh
source "$SCRIPT_DIR/lib/harden_suites.sh"
# shellcheck source=scripts/lib/proc_tree.sh
source "$SCRIPT_DIR/lib/proc_tree.sh"

if ! ROOT="$(git rev-parse --show-toplevel 2>/dev/null)"; then
    echo "harden-suites: not inside a git checkout." >&2
    exit 1
fi
STATE="$ROOT/.lucidos/harden-suites"
# Outside STATE, which every start clears: the record outlives the run.
RETRY_FILE="$ROOT/.lucidos/harden-retry"
REVIEW_OUT="$ROOT/.lucidos/other-model-review.out"
REVIEW_DONE="$ROOT/.lucidos/other-model-review.done"

usage() {
    awk 'NR == 1 { next } /^#/ { sub(/^# ?/, ""); print; next } { exit }' "${BASH_SOURCE[0]}"
}

# The kernel's start time for a pid, or nothing. A pid plus its start time
# names one process, so a recycled pid never matches (ADR 0025).
started_at() { # <pid>
    ps -o lstart= -p "$1" 2>/dev/null | sed 's/^ *//; s/ *$//'
}

# True when the recorded runner is still the process `start` recorded.
runner_alive() {
    local pid
    pid="$(cat "$STATE/pid" 2>/dev/null)" || return 1
    [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null || return 1
    [ "$(started_at "$pid")" = "$(cat "$STATE/pid-started" 2>/dev/null)" ]
}

descendants() { # <pid>
    ps -axo pid=,ppid= 2>/dev/null | proc_tree_walk "$1"
}

# The paths the branch changes against main, committed or not, which is what
# suite selection reads.
branch_paths() {
    git -C "$ROOT" diff --no-renames --name-only "$(git -C "$ROOT" merge-base main HEAD)" --
    git -C "$ROOT" ls-files -o --exclude-standard
}

# The tier this branch runs. No engine to ask and no HARDEN_TIER is the full
# tier.
resolve_tier() {
    local answer
    answer="$("${HS_LUCIDOS_CMD:-lucidos}" hardened expected-tier 2>/dev/null)" || answer=""
    hs_resolve_tier "$answer" "${HARDEN_TIER:-}"
}

recorded_tier() {
    hs_normalize_tier "$(cat "$STATE/tier" 2>/dev/null)"
}

# The browser specs a branch can break (touched_specs.sh), against main. What
# the selector says about the coverage map goes to the e2e-touched suite's log.
# HS_TOUCHED_SPECS_CMD replaces the selector in the tests.
select_touched_specs() { # <paths-file> <out-file> [<note-file>]
    "${HS_TOUCHED_SPECS_CMD:-ts_touched_specs}" "$ROOT" "$(git -C "$ROOT" merge-base main HEAD)" "$1" \
        "$(ts_coverage_map_path "$ROOT")" > "$2" 2> "${3:-/dev/null}"
}

retry_round() { [ -f "$STATE/retry" ]; }

selected_suites() {
    cat "$STATE/suites" 2>/dev/null
    if [ "$(cat "$STATE/mode" 2>/dev/null)" = early ] && ! retry_round && grep -q -x rust "$STATE/suites" 2>/dev/null; then
        echo driver
    fi
}

# A git failure prints a path that is never safe, so it voids the result.
changed_now() {
    hs_changed_paths "$ROOT" "$(cat "$STATE/start-sha")" "$STATE/untracked-at-start" \
        || echo "(git could not list the changed paths)"
}

# The build memory gate, or HS_MEMORY_GATE_CMD in the tests, which must never
# read the real host.
memory_gate() { # <label>
    "${HS_MEMORY_GATE_CMD:-$SCRIPT_DIR/build-memory-gate.sh}" --label "$1"
}

# A cargo suite first waits on the build memory gate (ADR 0351). A refusal is
# the suite's result, exit 72, and its command never runs.
run_suite() { # <suite>
    local suite=$1 cmd rc=0 passed_at=""
    if ! cmd="$(hs_suite_command "$suite" "$(cat "$STATE/mode")" "$STATE")"; then
        echo 1 > "$STATE/$suite.exit"
        return
    fi
    cat "$STATE/$suite.note" > "$STATE/$suite.log" 2>/dev/null || : > "$STATE/$suite.log"
    if hs_is_cargo_suite "$suite"; then
        memory_gate "the /harden $suite suite" >> "$STATE/$suite.log" 2>&1 || rc=$?
        passed_at="$(date +%s)"
    fi
    if [ "$rc" = 0 ]; then
        date +%s > "$STATE/$suite.start"
        (cd "$ROOT" && LUCIDOS_BUILD_MEMORY_GATE_PASSED_AT="$passed_at" bash -c "$cmd") >> "$STATE/$suite.log" 2>&1
        rc=$?
    fi
    date +%s > "$STATE/$suite.end"
    changed_now > "$STATE/$suite.changed"
    # The exit file goes last: its presence is what marks the suite done.
    echo "$rc" > "$STATE/$suite.exit"
}

# Start each suite in the background: the cargo suites one after another in
# one lane (two cargo runs in one worktree can OOM the host), the rest at once.
launch_suites() { # <newline-separated suites>
    local s cargo_lane="" other=""
    for s in $1; do
        if hs_is_cargo_suite "$s"; then
            cargo_lane="$cargo_lane $s"
        else
            other="$other $s"
        fi
    done
    if [ -n "$cargo_lane" ]; then
        (for s in $cargo_lane; do run_suite "$s"; done) &
    fi
    for s in $other; do run_suite "$s" & done
}

cmd_start() {
    local mode=normal suites early added="" s disabled tier label
    case "${1:-}" in
        --early) mode=early ;;
        "") ;;
        *) usage >&2; return 1 ;;
    esac
    if runner_alive; then
        echo "A suite run is still going. Stop it first: ./scripts/harden-suites.sh stop" >&2
        return 1
    fi
    if [ -n "$(git -C "$ROOT" status --porcelain --untracked-files=no)" ]; then
        echo "Uncommitted changes: commit them, then start the suites." >&2
        return 1
    fi

    rm -rf "$STATE"
    mkdir -p "$STATE"
    echo $$ > "$STATE/pid"
    started_at $$ > "$STATE/pid-started"
    echo "$mode" > "$STATE/mode"
    tier="$(resolve_tier)"
    echo "$tier" > "$STATE/tier"
    git -C "$ROOT" rev-parse HEAD > "$STATE/start-sha"
    hs_untracked_fingerprint "$ROOT" > "$STATE/untracked-at-start"
    branch_paths | sort -u > "$STATE/paths"
    { hs_compile_inputs "$ROOT"; hs_source_includes "$ROOT"; } | sort -u > "$STATE/compile-inputs"
    hs_source_includes "$ROOT" 'crates/lucidos-cli/*.rs' > "$STATE/cli-inputs"
    hs_vitest_inputs "$ROOT" > "$STATE/vitest-inputs"
    hs_generated_drift "$ROOT" > "$STATE/generated-drift"

    if ! disabled="$(hs_read_check "$ROOT")"; then
        printf '%s\n' "$disabled" > "$STATE/allowlist-disabled"
        echo "Safe-path allowlist disabled for this run, so any change voids it. These files may read a safe path:"
        printf '  %s\n' "$disabled"
    fi
    label="$mode"
    if [ "$tier" = fast ]; then label="$mode, fast tier"; fi

    if [ -s "$RETRY_FILE" ]; then
        suites="$(hs_retry_plan "$ROOT" "$STATE" < "$RETRY_FILE")"
        if [ -n "$suites" ]; then
            cp "$RETRY_FILE" "$STATE/retry"
            printf '%s\n' "$suites" > "$STATE/suites"
            echo "Suites ($label, a retry of what failed): $(tr '\n' ' ' < "$STATE/suites")"
            # A failed driver run reruns alone, once `wait` has joined the other-model review.
            launch_suites "$(printf '%s\n' "$suites" | grep -v -x driver)"
            wait
            return 0
        fi
        rm -f "$RETRY_FILE"
        echo "Nothing the last round failed is left to rerun, so this is a full run."
    fi

    # Only e2e-touched reads the specs, and it is no cargo suite, so the cargo
    # lane starts once, before the slow selection.
    early="$(hs_select_suites "$STATE/compile-inputs" "$STATE/cli-inputs" "$STATE/vitest-inputs" "$STATE/generated-drift" "$tier" /dev/null < "$STATE/paths" | sed '/^$/d')"
    launch_suites "$early"
    if ! select_touched_specs "$STATE/paths" "$STATE/touched-specs.tmp" "$STATE/e2e-touched.note"; then
        stop_tree "$(descendants $$ | grep -v -x $$)"
        echo "Could not select the touched browser specs." >&2
        return 1
    fi
    mv "$STATE/touched-specs.tmp" "$STATE/touched-specs"

    suites="$(hs_select_suites "$STATE/compile-inputs" "$STATE/cli-inputs" "$STATE/vitest-inputs" "$STATE/generated-drift" "$tier" "$STATE/touched-specs" < "$STATE/paths" | sed '/^$/d')"
    for s in $suites; do
        case $'\n'"$early"$'\n' in
            *$'\n'"$s"$'\n'*) ;;
            *) added="$added $s" ;;
        esac
    done
    # The suites file goes last: until it exists, the run is still setting up.
    printf '%s\n' "$suites" | sed '/^$/d' > "$STATE/suites.tmp"
    mv "$STATE/suites.tmp" "$STATE/suites"
    if [ ! -s "$STATE/suites" ]; then
        echo "No suites selected for this diff."
        return 0
    fi
    echo "Suites ($label): $(tr '\n' ' ' < "$STATE/suites")"

    launch_suites "$added"
    wait
    return 0
}

# Stop each pid and every process it left behind, TERM first and KILL for a
# survivor. The snapshot carries start times: once a parent dies its children
# belong to launchd, and a fresh walk would miss them.
stop_tree() { # <newline-separated pids>
    local p snapshot="" survivors
    for p in $1; do
        snapshot="$snapshot$p $(started_at "$p")
"
    done
    for p in $(still_running "$snapshot"); do kill -TERM "$p" 2>/dev/null; done
    for _ in 1 2 3 4 5 6 7 8 9 10; do
        survivors="$(still_running "$snapshot")"
        [ -z "$survivors" ] && break
        sleep 1
    done
    for p in $survivors; do kill -KILL "$p" 2>/dev/null; done
}

cmd_stop() {
    if ! runner_alive; then
        echo "No suite run is going."
        return 0
    fi
    stop_tree "$(descendants "$(cat "$STATE/pid")")"
    touch "$STATE/stopped"
    echo "Stopped the suite run."
}

# Print each snapshot pid that is still the same process.
still_running() { # <"pid start-time" lines>
    local pid started
    printf '%s' "$1" | while read -r pid started; do
        [ -n "$pid" ] && [ "$(started_at "$pid")" = "$started" ] && echo "$pid"
    done
}

# The suites the runner owns: the selection, plus the driver once `wait`
# started it.
running_suites() {
    cat "$STATE/suites"
    [ -f "$STATE/driver.started" ] && echo driver
}

all_done() {
    local s
    runner_alive || return 0
    [ -f "$STATE/suites" ] || return 1
    for s in $(running_suites); do
        [ -f "$STATE/$s.exit" ] || return 1
    done
    return 0
}

# Sleep until every suite is done or the deadline passes. The deadline is on
# bash's own clock, so a slow host's checks cannot stretch the budget.
join_until() { # <deadline in $SECONDS>
    while ! all_done && [ "$SECONDS" -lt "$1" ]; do
        sleep 1
    done
    all_done
}

# Start the driver run as its own process, so it outlives the `wait` that
# started it. It takes over the runner record, so `stop` and `wait` reach it.
start_driver() {
    local pid
    # The marker goes first, so the runner record never names a driver run
    # that the selection lacks.
    touch "$STATE/driver.started"
    bash "${BASH_SOURCE[0]}" run-driver > /dev/null 2>&1 &
    pid=$!
    echo "$pid" > "$STATE/pid"
    started_at "$pid" > "$STATE/pid-started"
}

driver_due() {
    selected_suites | grep -q -x driver && [ ! -f "$STATE/driver.started" ] && [ ! -f "$STATE/stopped" ] \
        && { ! grep -q -x rust "$STATE/suites" || [ "$(suite_state rust)" = PASS ]; }
}

cmd_wait() {
    local budget=$HS_WAIT_BUDGET review_abandoned="" deadline
    while [ $# -gt 0 ]; do
        case "$1" in
            --review-abandoned) review_abandoned=1 ;;
            --budget) shift; budget=$1 ;;
            *) usage >&2; return 1 ;;
        esac
        shift
    done
    if [ ! -f "$STATE/suites" ] && ! runner_alive; then
        echo "MISSING: no suite run recorded. Start one: ./scripts/harden-suites.sh start" >&2
        return 2
    fi
    deadline=$((SECONDS + budget))
    if ! join_until "$deadline"; then
        echo "Still running after ${budget}s. Re-issue: ./scripts/harden-suites.sh wait"
        return 3
    fi
    if driver_due; then
        if [ -f "$REVIEW_OUT" ] && [ ! -f "$REVIEW_DONE" ] && [ -z "$review_abandoned" ]; then
            echo "The other-model review is still running. Join it (harden.md Phase 3), then re-issue wait."
            echo "If Phase 3 abandoned it, re-issue: ./scripts/harden-suites.sh wait --review-abandoned"
            return 3
        fi
        echo "Starting the Codex driver tests alone."
        start_driver
        if ! join_until "$deadline"; then
            echo "The Codex driver tests are still running. Re-issue: ./scripts/harden-suites.sh wait"
            return 3
        fi
    fi
    cmd_verdict
}

# Why a path on the allowlist voided the run: the read check turned the
# allowlist off at start. Prints nothing for any other unsafe path.
allowlist_off_note() { # <path>
    if [ ! -f "$STATE/allowlist-disabled" ] || ! hs_is_safe_path "$1" \
        || grep -q -x -F -- "$1" "$STATE/compile-inputs" 2>/dev/null; then
        return 0
    fi
    printf ' (the safe-path allowlist is off: %s may read a safe path)' \
        "$(cut -d' ' -f1 "$STATE/allowlist-disabled" | tr '\n' ',' | sed 's/,$//; s/,/, /g')"
}

# Print the state of one suite: PASS, SKIP <reason>, FAIL, VOID <reason>,
# RUNNING or MISSING.
suite_state() { # <suite>
    local suite=$1 disabled=0 unsafe
    if [ ! -f "$STATE/$suite.exit" ]; then
        if [ -f "$STATE/stopped" ]; then
            echo "VOID stopped before it finished"
        elif runner_alive; then
            echo RUNNING
        else
            echo MISSING
        fi
        return
    fi
    [ -f "$STATE/allowlist-disabled" ] && disabled=1
    unsafe="$({ cat "$STATE/$suite.changed" 2>/dev/null; changed_now; } | sort -u \
        | hs_unsafe_paths "$STATE/compile-inputs" "$disabled" | head -1)"
    if [ -n "$unsafe" ]; then
        echo "VOID $unsafe changed since the start commit$(allowlist_off_note "$unsafe")"
    elif [ "$(cat "$STATE/$suite.exit")" = 72 ] ||
        grep -q '^ERROR: build refused on host memory' "$STATE/$suite.log" 2>/dev/null; then
        # A gate nested inside `make` refuses with 72, and make exits 2.
        echo "VOID refused by the build memory gate, see $STATE/$suite.log"
    elif hs_is_e2e_suite "$suite" && [ "$(cat "$STATE/$suite.exit")" = "$E2E_LOCK_BUSY_EXIT" ]; then
        # ADR 0541: a busy e2e lock is given up, and the main check runs the specs.
        echo "SKIP every e2e slot was busy; the main check runs these specs after landing"
    elif [ "$(cat "$STATE/$suite.exit")" != 0 ]; then
        echo "FAIL see $STATE/$suite.log"
    elif { hs_is_filtered_suite "$suite" || { hs_names_engine_tests "$suite" && [ -s "$STATE/$suite.retry" ]; }; } \
        && ! grep -q -E '^running [1-9]' "$STATE/$suite.log"; then
        echo "FAIL ran no tests"
    else
        echo PASS
    fi
}

cmd_verdict() {
    local s state now fail=0 rerun=0
    if [ ! -f "$STATE/suites" ]; then
        if runner_alive; then
            echo "RUNNING: the run is still setting up"
            return 3
        fi
        echo "MISSING: no suite run recorded"
        return 2
    fi
    # A fix can widen the selection, and no recorded suite would notice. A
    # retry round runs a subset by design, and the full run after it selects.
    if ! retry_round; then
        hs_generated_drift "$ROOT" > "$STATE/generated-drift-now"
        branch_paths | sort -u > "$STATE/paths-now"
        select_touched_specs "$STATE/paths-now" "$STATE/touched-specs-now" \
            || echo "(the touched specs could not be selected)" > "$STATE/touched-specs-now"
        now="$(hs_select_suites "$STATE/compile-inputs" "$STATE/cli-inputs" "$STATE/vitest-inputs" "$STATE/generated-drift-now" "$(recorded_tier)" "$STATE/touched-specs-now" < "$STATE/paths-now")"
        if [ "$now" != "$(cat "$STATE/suites")" ]; then
            echo "selection VOID the diff now selects: $(printf '%s' "$now" | tr '\n' ' ')"
            rerun=1
        fi
    fi
    : > "$STATE/failed"
    for s in $(selected_suites); do
        state="$(suite_state "$s")"
        echo "$s $state"
        case "$state" in
            PASS | SKIP*) ;;
            FAIL*) fail=1; failure_record "$s" >> "$STATE/failed" ;;
            RUNNING) return 3 ;;
            *) rerun=1 ;;
        esac
    done
    if [ "$fail" = 1 ]; then
        mv "$STATE/failed" "$RETRY_FILE"
        echo "verdict: FAIL. Fix, commit, and return to Phase 1. The next start reruns what failed first."
        return 1
    elif [ "$rerun" = 1 ]; then
        echo "verdict: RERUN. Start a normal run: ./scripts/harden-suites.sh start"
        return 2
    fi
    rm -f "$RETRY_FILE"
    if retry_round; then
        echo "verdict: RERUN. What failed now passes. Start a normal run for the full set: ./scripts/harden-suites.sh start"
        return 2
    fi
    echo "verdict: PASS"
    return 0
}

# The specs a touched-spec suite ran.
suite_specs() { # <suite>
    if [ -s "$STATE/$1.retry" ]; then
        cat "$STATE/$1.retry"
    elif [ "$1" = e2e-touched-webkit ]; then
        hs_webkit_specs < "$STATE/touched-specs"
    else
        cat "$STATE/touched-specs"
    fi
}

# The record lines for one failed suite: "<suite> <test>" for each test its log
# names, or "<suite>" alone to rerun it whole. A touched-spec suite whose log
# names no spec reruns every spec it ran.
failure_record() { # <suite>
    local names
    names="$(hs_failed_tests "$1" "$STATE/$1.log")"
    if [ -z "$names" ] && hs_is_e2e_suite "$1"; then
        names="$(suite_specs "$1" 2>/dev/null)"
    fi
    if [ -z "$names" ] || printf '%s\n' "$names" | grep -q -x -F '?'; then
        echo "$1"
    else
        printf '%s\n' "$names" | sed '/^$/d' | sed "s/^/$1 /"
    fi
}

# Each suite's start, end and exit code, as one JSON object for the
# HardenRunCompleted payload. A suite that never started has nulls.
cmd_timings() {
    local s first=1 start end rc round=full
    if [ ! -f "$STATE/suites" ]; then
        echo '{}'
        return 0
    fi
    retry_round && round=retry
    printf '{"round": "%s", "mode": "%s", "suites": {' "$round" "$(cat "$STATE/mode")"
    for s in $(running_suites); do
        start="$(cat "$STATE/$s.start" 2>/dev/null)"
        end="$(cat "$STATE/$s.end" 2>/dev/null)"
        rc="$(cat "$STATE/$s.exit" 2>/dev/null)"
        [ "$first" = 1 ] || printf ', '
        first=0
        printf '"%s": {"start": %s, "end": %s, "exit": %s}' "$s" "${start:-null}" "${end:-null}" "${rc:-null}"
    done
    echo '}}'
}

case "${1:-}" in
    start) shift; cmd_start "$@" ;;
    stop) cmd_stop ;;
    wait) shift; cmd_wait "$@" ;;
    run-driver) run_suite driver ;;
    verdict) cmd_verdict ;;
    timings) cmd_timings ;;
    -h | --help) usage ;;
    *) usage >&2; exit 1 ;;
esac
