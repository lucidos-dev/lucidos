#!/usr/bin/env bash
#
# harden-suites.sh: run the `/harden` Phase 4.5 test suites, early or late,
# and say whether their result still describes the branch (ADR 0292).
#
#   ./scripts/harden-suites.sh start --early   # at the Phase 1 kickoff
#   ./scripts/harden-suites.sh stop            # before a fix the result cannot survive
#   ./scripts/harden-suites.sh wait            # at Phase 4.5: join, then the verdict
#                   [--codex-abandoned]        #   Phase 3 gave up on the Codex review
#                   [--budget <seconds>]       #   how long to wait (default 480)
#   ./scripts/harden-suites.sh verdict         # the verdict alone
#   ./scripts/harden-suites.sh start           # a normal run, when the verdict says RERUN
#
# `start` selects suites from `git diff main...HEAD`, runs them, and records
# the start commit. An early run skips the two Codex driver modules, and
# `wait` runs them alone once the Codex review is joined.
#
# The verdict prints one line per suite: PASS, FAIL, VOID <reason>, RUNNING or
# MISSING. An early result stays valid only while every path changed since the
# start commit is on HARDEN_SAFE_PATHS (scripts/lib/harden_suites.sh). Each
# suite also stamps the changed paths when it exits, so an edit made while it
# ran voids it even if the edit was later undone.
#
# A cargo suite waits on the build memory gate before it starts, and exits 72
# when the host never recovers (ADR 0351).
#
# The run stays in the caller's process group, so the turn-end teardown still
# reaches it. `stop` walks the pid tree from the recorded pid, never by name.
#
# State and logs: .lucidos/harden-suites/ in the worktree (gitignored).
#
# Exit status: 0 every suite PASS, 1 a FAIL or bad usage, 2 RERUN needed
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
CODEX_OUT="$ROOT/.lucidos/codex-review.out"
CODEX_DONE="$ROOT/.lucidos/codex-review.done"

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

selected_suites() {
    cat "$STATE/suites" 2>/dev/null
    if [ "$(cat "$STATE/mode" 2>/dev/null)" = early ] && grep -q -x rust "$STATE/suites" 2>/dev/null; then
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
    if ! cmd="$(hs_suite_command "$suite" "$(cat "$STATE/mode")" "$STATE/paths")"; then
        echo 1 > "$STATE/$suite.exit"
        return
    fi
    : > "$STATE/$suite.log"
    if hs_is_cargo_suite "$suite"; then
        memory_gate "the /harden $suite suite" >> "$STATE/$suite.log" 2>&1 || rc=$?
        passed_at="$(date +%s)"
    fi
    if [ "$rc" = 0 ]; then
        (cd "$ROOT" && LUCIDOS_BUILD_MEMORY_GATE_PASSED_AT="$passed_at" bash -c "$cmd") >> "$STATE/$suite.log" 2>&1
        rc=$?
    fi
    changed_now > "$STATE/$suite.changed"
    # The exit file goes last: its presence is what marks the suite done.
    echo "$rc" > "$STATE/$suite.exit"
}

cmd_start() {
    local mode=normal suites cargo_lane="" other="" s disabled
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
    git -C "$ROOT" rev-parse HEAD > "$STATE/start-sha"
    hs_untracked_fingerprint "$ROOT" > "$STATE/untracked-at-start"
    branch_paths | sort -u > "$STATE/paths"
    { hs_compile_inputs "$ROOT"; hs_source_includes "$ROOT"; } | sort -u > "$STATE/compile-inputs"
    hs_source_includes "$ROOT" 'crates/lucidos-cli/*.rs' > "$STATE/cli-inputs"
    hs_vitest_inputs "$ROOT" > "$STATE/vitest-inputs"

    if ! disabled="$(hs_read_check "$ROOT")"; then
        printf '%s\n' "$disabled" > "$STATE/allowlist-disabled"
        echo "Safe-path allowlist disabled for this run, so any change voids it. These files may read a safe path:"
        printf '  %s\n' "$disabled"
    fi

    suites="$(hs_select_suites "$STATE/compile-inputs" "$STATE/cli-inputs" "$STATE/vitest-inputs" < "$STATE/paths")"
    # The suites file goes last: until it exists, the run is still setting up.
    printf '%s\n' "$suites" | sed '/^$/d' > "$STATE/suites.tmp"
    mv "$STATE/suites.tmp" "$STATE/suites"
    if [ ! -s "$STATE/suites" ]; then
        echo "No suites selected for this diff."
        return 0
    fi
    echo "Suites ($mode): $(tr '\n' ' ' < "$STATE/suites")"

    for s in $suites; do
        if hs_is_cargo_suite "$s"; then
            cargo_lane="$cargo_lane $s"
        else
            other="$other $s"
        fi
    done
    (for s in $cargo_lane; do run_suite "$s"; done) &
    for s in $other; do run_suite "$s" & done
    wait
    return 0
}

cmd_stop() {
    local p i snapshot="" survivors
    if ! runner_alive; then
        echo "No suite run is going."
        return 0
    fi
    # Snapshot the tree with start times first. Once the runner dies, its
    # children belong to launchd and a fresh walk would miss them.
    for p in $(descendants "$(cat "$STATE/pid")"); do
        snapshot="$snapshot$p $(started_at "$p")
"
    done
    for p in $(printf '%s' "$snapshot" | cut -d' ' -f1); do kill -TERM "$p" 2>/dev/null; done
    for i in 1 2 3 4 5 6 7 8 9 10; do
        survivors="$(still_running "$snapshot")"
        [ -z "$survivors" ] && break
        sleep 1
    done
    for p in $survivors; do kill -KILL "$p" 2>/dev/null; done
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

all_done() {
    local s
    runner_alive || return 0
    [ -f "$STATE/suites" ] || return 1
    while IFS= read -r s; do
        [ -f "$STATE/$s.exit" ] || return 1
    done < "$STATE/suites"
    return 0
}

cmd_wait() {
    local budget=480 codex_abandoned="" i=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --codex-abandoned) codex_abandoned=1 ;;
            --budget) shift; budget=$1 ;;
            *) usage >&2; return 1 ;;
        esac
        shift
    done
    if [ ! -f "$STATE/suites" ] && ! runner_alive; then
        echo "MISSING: no suite run recorded. Start one: ./scripts/harden-suites.sh start" >&2
        return 2
    fi
    while ! all_done && [ "$i" -lt "$budget" ]; do
        sleep 1
        i=$((i + 1))
    done
    if ! all_done; then
        echo "Still running after ${budget}s. Re-issue: ./scripts/harden-suites.sh wait"
        return 3
    fi
    if selected_suites | grep -q -x driver && [ ! -f "$STATE/driver.exit" ] && [ ! -f "$STATE/stopped" ] \
        && [ "$(suite_state rust)" = PASS ]; then
        if [ -f "$CODEX_OUT" ] && [ ! -f "$CODEX_DONE" ] && [ -z "$codex_abandoned" ]; then
            echo "The Codex review is still running. Join it (harden.md Phase 3), then re-issue wait."
            echo "If Phase 3 abandoned it, re-issue: ./scripts/harden-suites.sh wait --codex-abandoned"
            return 3
        fi
        # A long join plus a driver run that relinks the engine can outlast
        # one 600 s Bash call, so a slow join hands the driver to the next wait.
        if [ "$i" -gt 120 ]; then
            echo "Suites joined. Re-issue wait to run the Codex driver tests alone."
            return 3
        fi
        echo "Running the Codex driver tests alone."
        run_suite driver
    fi
    cmd_verdict
}

# Print the state of one suite: PASS, FAIL, VOID <reason>, RUNNING or MISSING.
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
        echo "VOID $unsafe changed since the start commit"
    elif [ "$(cat "$STATE/$suite.exit")" = 72 ] ||
        grep -q '^ERROR: build refused on host memory' "$STATE/$suite.log" 2>/dev/null; then
        # A gate nested inside `make` refuses with 72, and make exits 2.
        echo "VOID refused by the build memory gate, see $STATE/$suite.log"
    elif [ "$(cat "$STATE/$suite.exit")" != 0 ]; then
        echo "FAIL see $STATE/$suite.log"
    elif hs_is_filtered_suite "$suite" && ! grep -q -E '^running [1-9]' "$STATE/$suite.log"; then
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
    # A fix can widen the selection, and no recorded suite would notice.
    now="$(branch_paths | sort -u | hs_select_suites "$STATE/compile-inputs" "$STATE/cli-inputs" "$STATE/vitest-inputs")"
    if [ "$now" != "$(cat "$STATE/suites")" ]; then
        echo "selection VOID the diff now selects: $(printf '%s' "$now" | tr '\n' ' ')"
        rerun=1
    fi
    for s in $(selected_suites); do
        state="$(suite_state "$s")"
        echo "$s $state"
        case "$state" in
            PASS) ;;
            FAIL*) fail=1 ;;
            RUNNING) return 3 ;;
            *) rerun=1 ;;
        esac
    done
    if [ "$fail" = 1 ]; then
        echo "verdict: FAIL. Fix, commit, and return to Phase 1."
        return 1
    elif [ "$rerun" = 1 ]; then
        echo "verdict: RERUN. Start a normal run: ./scripts/harden-suites.sh start"
        return 2
    fi
    echo "verdict: PASS"
    return 0
}

case "${1:-}" in
    start) shift; cmd_start "$@" ;;
    stop) cmd_stop ;;
    wait) shift; cmd_wait "$@" ;;
    verdict) cmd_verdict ;;
    -h | --help) usage ;;
    *) usage >&2; exit 1 ;;
esac
