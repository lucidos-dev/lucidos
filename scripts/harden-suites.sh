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
#                   [--include-suite <name>]  # repeat to add canonical suites
#   ./scripts/harden-suites.sh timings         # each suite's start, end and exit, as JSON
#
# `start` selects suites from `git diff main...HEAD`, runs them, and records
# the start commit. The Rust suites follow the crates the diff reaches in the
# workspace's dependency graph, which `start` reads once from `cargo metadata`
# (hs_reached_crates). An early run skips the two Codex driver modules. Once the
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
# Includes add to automatic selection in both tiers, even when the main check
# covers a requested suite. Names are validated before evidence is replaced.
# Requests persist in .lucidos/harden-included-suites for this worktree and are
# copied to each run's included-suites. Ordinary starts and retries retain them.
# Existing retries keep their subset; a new request forces a full round.
# Commands remain defined by hs_suite_command, with Cargo in its serial lane.
#
# A FAIL verdict records what failed in .lucidos/harden-retry: engine tests and
# browser specs by name, other failures and unfinished suites whole. The next
# `start` retries the obligations still selected by the current change.
# Browser retries select specs before launching any suites. Passing retries
# complete the harden when only standalone tests changed since the last full
# round started. Otherwise they say RERUN for the full set (ADR 0536 and its
# amendment).
#
# A passing touched-spec suite records each spec it ran in .lucidos/harden-carried,
# with the commit it ran at. The next round leaves out a spec whose record is
# valid: every path changed since that commit is a safe path. A suite with no
# specs left reads PASS carried from <commit>. A FAIL, SKIP or VOID records
# nothing.
#
# The touched specs a WebKit project runs get their own suite, e2e-touched-webkit.
# Every host sends it to GitHub as the run starts, beside e2e-touched.
# The rest run on this host.
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
# When a selected suite reads codegen's output (`hs_waits_for_codegen`), the
# cargo lane first runs codegen-step, behind the same gate. Those suites
# wait for it, then run outside the lane and never compile. A failed step fails
# each of them, and a refused gate voids them.
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
# `wait` reports a FAIL as soon as one suite fails, while the rest may still
# run. An unfinished suite is never a pass: the next round owes that suite whole.
#
# Exit status: 0 every suite PASS or SKIP, 1 a FAIL or bad usage, 2 RERUN needed
# (a VOID or MISSING line, and no FAIL), 3 still running.

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/lib/harden_suites.sh
source "$SCRIPT_DIR/lib/harden_suites.sh"
# shellcheck source=scripts/lib/proc_tree.sh
source "$SCRIPT_DIR/lib/proc_tree.sh"
# shellcheck source=scripts/lib/ps_tools.sh
source "$SCRIPT_DIR/lib/ps_tools.sh"
ps_tools_env
# shellcheck source=scripts/lib/flock.sh
source "$SCRIPT_DIR/lib/flock.sh"

if ! ROOT="$(git rev-parse --show-toplevel 2>/dev/null)"; then
    echo "harden-suites: not inside a git checkout." >&2
    exit 1
fi
STATE="$ROOT/.lucidos/harden-suites"
START_RECORD="$ROOT/.lucidos/harden-start"
# Outside STATE, which every start clears: the record outlives the run.
RETRY_FILE="$ROOT/.lucidos/harden-retry"
RETRY_BASE="$ROOT/.lucidos/harden-retry-base"
CARRY_FILE="$ROOT/.lucidos/harden-carried"
INCLUDE_FILE="$ROOT/.lucidos/harden-included-suites"
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

startup_pending() {
    [ "$(cat "$START_RECORD.status" 2>/dev/null)" = pending ] && lock_is_held "$START_RECORD.lock"
}

startup_refused() {
    local status
    status="$(cat "$START_RECORD.status" 2>/dev/null)"
    [ "$status" = refused ] || { [ "$status" = pending ] && ! lock_is_held "$START_RECORD.lock"; }
}

descendants() { # <pid>
    ps -axo pid=,ppid= 2>/dev/null | proc_tree_walk "$1"
}

# The paths the branch changes against main, committed or not, which is what
# suite selection reads.
branch_paths() {
    local marker overlap
    if [ -n "${HS_MAIN_CHECK_COVERED:-}" ] &&
        [[ "$(git -C "$ROOT" symbolic-ref --quiet --short HEAD)" == lane/* ]] &&
        git -C "$ROOT" diff --quiet HEAD -- &&
        [ "$("${HS_LUCIDOS_CMD:-lucidos}" hardened tier 2>/dev/null)" = full ] &&
        marker=$("${HS_LUCIDOS_CMD:-lucidos}" hardened sha 2>/dev/null) &&
        overlap=$(cd "$ROOT" && "$SCRIPT_DIR/harden-scope.sh" --lane-suite-paths "$marker"); then
        printf '%s\n' "$overlap"
        git -C "$ROOT" ls-files -o --exclude-standard
        return
    fi
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

# Whether a main check covers this workspace, as the engine says. No engine,
# an older one, or any other answer is `off`: the round then keeps every suite.
resolve_main_check_setting() {
    "${HS_LUCIDOS_CMD:-lucidos}" main-check setting 2>/dev/null | head -1
}

# Exports HS_MAIN_CHECK_COVERED from the setting `start` recorded, so the
# verdict's re-selection agrees with the start's.
use_main_check_setting() {
    HS_MAIN_CHECK_COVERED=""
    if hs_main_check_covers "$(cat "$1/main-check-setting" 2>/dev/null)"; then
        HS_MAIN_CHECK_COVERED=1
    fi
    export HS_MAIN_CHECK_COVERED
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
    if [ "$(cat "$STATE/mode" 2>/dev/null)" = early ] && ! retry_round && grep -q -x engine "$STATE/suites" 2>/dev/null; then
        grep -q -x driver "$STATE/suites" || echo driver
    fi
}

# Keep automatic order and add each required suite once. Browser suites need
# the slow spec selection; Cargo includes always join the first lane.
select_with_includes() {
    local s included_suites=${HS_INCLUDED_SUITES_INPUT:-$STATE/included-suites}
    {
        hs_select_suites "$@"
        [ -f "$included_suites" ] || return 0
        while IFS= read -r s; do
            if [ "${6:-}" = /dev/null ] && hs_is_e2e_suite "$s"; then continue; fi
            echo "$s"
        done < "$included_suites"
    } | awk 'NF && !seen[$0]++'
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

# Run one step's command from the root, appending to <name>.log. A gate label
# first waits on the build memory gate (ADR 0351): a refusal is the result,
# exit 72, and the command never runs.
run_step() { # <name> <command> [<gate label>]
    local name=$1 cmd=$2 rc=0 passed_at=""
    if [ -n "${3:-}" ]; then
        memory_gate "$3" >> "$STATE/$name.log" 2>&1 || rc=$?
        passed_at="$(date +%s)"
    fi
    if [ "$rc" = 0 ]; then
        date +%s > "$STATE/$name.start"
        (
            cd "$ROOT" || exit 1
            if [ -n "${3:-}" ] && [ -z "${HS_STUB_CMD:-}" ]; then
                export LUCIDOS_SLOT_RECHECK_MEMORY=1 LUCIDOS_HARDEN_BUILD_CHECKED=1
                case "$cmd" in
                    "cargo "* | "make test-crates")
                        exec "$SCRIPT_DIR/with-build-slot.sh" --label "$3" -- bash -c "$cmd" ;;
                esac
            fi
            LUCIDOS_BUILD_MEMORY_GATE_PASSED_AT="$passed_at" exec bash -c "$cmd"
        ) >> "$STATE/$name.log" 2>&1
        rc=$?
    fi
    date +%s > "$STATE/$name.end"
    changed_now > "$STATE/$name.changed"
    # The exit file goes last: its presence is what marks the step done.
    echo "$rc" > "$STATE/$name.exit"
}

# The lane's codegen step. Its exit file is what the waiting suites read.
run_codegen_step() {
    local cmd
    : > "$STATE/codegen-step.log"
    if ! cmd="$(hs_suite_command codegen-step "$(cat "$STATE/mode")" "$STATE")"; then
        echo 1 > "$STATE/codegen-step.exit"
        return
    fi
    run_step codegen-step "$cmd" "the /harden codegen step"
}

# A suite that waits for the codegen step takes a failed step's exit code as
# its own, so a refused gate (72) voids it and any other failure fails it.
run_suite() { # <suite> [checked-knowhow]
    local suite=$1 cmd gate="" codegen_rc
    if ! cmd="$(hs_suite_command "$suite" "$(cat "$STATE/mode")" "$STATE" "${2:-}")"; then
        echo 1 > "$STATE/$suite.exit"
        return
    fi
    cat "$STATE/$suite.note" > "$STATE/$suite.log" 2>/dev/null || : > "$STATE/$suite.log"
    if hs_waits_for_codegen "$suite"; then
        while [ ! -f "$STATE/codegen-step.exit" ]; do
            # A lane gone with no exit file leaves this suite MISSING, so the verdict reruns.
            if ! kill -0 "$(cat "$STATE/codegen-step.pid" 2>/dev/null)" 2>/dev/null; then
                [ -f "$STATE/codegen-step.exit" ] || return
            fi
            sleep 1
        done
        codegen_rc="$(cat "$STATE/codegen-step.exit")"
        if [ "$codegen_rc" != 0 ]; then
            cmd="$(printf 'echo %q >&2; exit %q' "The codegen step failed, see $STATE/codegen-step.log" "$codegen_rc")"
        fi
    fi
    hs_is_cargo_suite "$suite" && gate="the /harden $suite suite"
    run_step "$suite" "$cmd" "$gate"
}

# Resolve the same Clippy scope used by the suite. Other Cargo suites are
# headless except app-lib. Codegen can compile on a cache miss.
check_build_prerequisites() { # <selected suites> <staged state>
    local suites=$1 state=$2 suite scope compile=""
    local check=${HS_BUILD_PREREQUISITES_CMD:-$SCRIPT_DIR/build-prerequisites.sh}
    local suite_words
    suite_words="$(printf '%s' "$suites" | tr '\n' ' ')"
    if [ -n "${HS_STUB_CMD:-}" ] && [ -z "${HS_BUILD_PREREQUISITES_CMD:-}" ]; then return 0; fi
    for suite in $suites; do
        if [ "$suite" = lint-rust ]; then
            scope="$(hs_lint_scope "$state")"
            case " $suite_words " in
                *" app-lib "*)
                    if [ "$scope" != --workspace ]; then scope="$scope -p lucidos-app"; fi ;;
            esac
            # shellcheck disable=SC2086 # package flags emitted by hs_lint_scope
            "$check" -- $scope --all-features
            return
        fi
        hs_is_cargo_suite "$suite" && compile=1
    done
    if printf '%s\n' "$suites" | grep -qx app-lib; then
        "$check" --selection desktop
    elif [ -n "$compile" ] || ! hs_codegen_fingerprint_matches "$ROOT" ||
        { hs_needs_codegen "$suites" && ! "${HS_CODEGEN:-$SCRIPT_DIR/codegen.sh}" --cache-dir >/dev/null; }; then
        "$check" --selection headless
    fi
}

# Complete all selected independent work before any generated-surface discovery
# or build launch. Keep ordinary suite timings and exits, including each failure.
run_independent_suites() { # <newline-separated suites> <checked-knowhow>
    local s failed=0
    for s in $1; do
        hs_is_independent_suite "$s" || continue
        date +%s > "$STATE/$s.queued"
        run_suite "$s" "$2"
        [ "$(cat "$STATE/$s.exit" 2>/dev/null)" = 0 ] || failed=1
    done
    return "$failed"
}

# Start each suite in the background: the cargo suites one after another in
# one lane (two cargo runs in one worktree can OOM the host), the rest at once.
# The lane runs the codegen step first, once per run, when a suite waits for it.
launch_suites() { # <newline-separated suites>
    local s cargo_lane="" other="" codegen=""
    for s in $1; do
        [ ! -f "$STATE/$s.exit" ] || continue
        date +%s > "$STATE/$s.queued"
        if hs_is_cargo_suite "$s"; then
            cargo_lane="$cargo_lane $s"
        else
            other="$other $s"
        fi
    done
    if hs_needs_codegen "$1" && [ ! -f "$STATE/codegen-step.queued" ]; then
        date +%s > "$STATE/codegen-step.queued"
        codegen=1
    fi
    if [ -n "$cargo_lane$codegen" ]; then
        (
            [ -z "$codegen" ] || run_codegen_step
            for s in $cargo_lane; do run_suite "$s"; done
        ) &
        [ -z "$codegen" ] || echo $! > "$STATE/codegen-step.pid"
    fi
    for s in $other; do run_suite "$s" & done
}

cmd_start() {
    local mode=normal suites early added="" s disabled tier label includes="" new_includes=""
    while [ $# -gt 0 ]; do
        case "$1" in
            --early) mode=early ;;
            --include-suite)
                shift
                if [ $# -eq 0 ]; then usage >&2; return 1; fi
                hs_is_suite "$1" || return 1
                includes="$includes$1"$'\n'
                ;;
            *) usage >&2; return 1 ;;
        esac
        shift
    done
    includes="$( { cat "$INCLUDE_FILE" 2>/dev/null; printf '%s' "$includes"; } | sed '/^$/d' | sort -u)"
    for s in $includes; do hs_is_suite "$s" || return 1; done
    [ "$includes" = "$(cat "$INCLUDE_FILE" 2>/dev/null)" ] || new_includes=1
    mkdir -p "$ROOT/.lucidos"
    if ! flock_try 8 "$START_RECORD.lock" ex; then
        echo "A suite run is still going. Stop it first: ./scripts/harden-suites.sh stop" >&2
        return 1
    fi
    if runner_alive; then
        echo "A suite run is still going. Stop it first: ./scripts/harden-suites.sh stop" >&2
        return 1
    fi
    if [ -n "$(git -C "$ROOT" status --porcelain --untracked-files=no)" ]; then
        echo "Uncommitted changes: commit them, then start the suites." >&2
        return 1
    fi

    (
    # This owner survives a killed CLI supervisor. Keep its lock until all
    # startup and suite work ends; descendants close it so stop can drain them.
    local owner
    owner=${BASHPID:-$(exec sh -c 'printf "%s" "$PPID"')}
    echo "$owner" > "$START_RECORD.pid"
    started_at "$owner" > "$START_RECORD.started"
    echo pending > "$START_RECORD.status"
    (
    exec 8>&-
    # Run independent gates before generated drift (which may compile) and
    # before replacing any evidence, retries or durable include obligations.
    # Fixtures replace the checks while exercising real suite selection.
    local gated_sha planned retry_plan="" startup_failed=0 independent_failed=0
    gated_sha="$(git -C "$ROOT" rev-parse HEAD)"
    if ! (cd "$ROOT" && "${HS_GATES_CMD:-$SCRIPT_DIR/harden-gates.sh}"); then
        startup_failed=1
        echo "Independent hardening gates failed." >&2
    fi

    # Stage selection inputs so prerequisite failures preserve the old run.
    local startup_state
    startup_state="$(mktemp -d "$STATE.start.XXXXXX")" || return 1
    trap 'rm -rf -- "$startup_state"' EXIT
    printf '%s\n' "$includes" | sed '/^$/d' > "$startup_state/included-suites"
    tier="$(resolve_tier)"
    resolve_main_check_setting > "$startup_state/main-check-setting"
    use_main_check_setting "$startup_state"
    git -C "$ROOT" rev-parse HEAD > "$startup_state/start-sha"
    hs_untracked_fingerprint "$ROOT" > "$startup_state/untracked-at-start"
    branch_paths | sort -u > "$startup_state/paths"
    { hs_compile_inputs "$ROOT"; hs_source_includes "$ROOT"; } | sort -u > "$startup_state/compile-inputs"
    hs_source_includes "$ROOT" 'crates/lucidos-cli/*.rs' > "$startup_state/cli-inputs"
    hs_test_inputs "$ROOT" > "$startup_state/test-inputs"
    hs_vitest_inputs "$ROOT" > "$startup_state/vitest-inputs"
    : > "$startup_state/generated-drift"
    local codegen=${HS_CODEGEN:-$SCRIPT_DIR/codegen.sh}
    "$codegen" --fingerprint > "$startup_state/codegen-fingerprint"
    # Metadata enumerates manifests without compiling.
    hs_crate_graph "$ROOT" > "$startup_state/crate-graph"
    hs_compile_owners "$ROOT" "$startup_state/crate-graph" > "$startup_state/compile-owners"

    if [ -s "$RETRY_FILE" ] && [ -z "$new_includes" ]; then
        while read -r s _; do
            hs_is_e2e_suite "$s" || continue
            if ! select_touched_specs "$startup_state/paths" "$startup_state/touched-specs.tmp" "$startup_state/e2e-touched.note"; then
                echo "Could not select the touched browser specs for the retry; the failure record is unchanged." >&2
                echo refused > "$START_RECORD.status"
                return 1
            fi
            mv "$startup_state/touched-specs.tmp" "$startup_state/touched-specs"
            break
        done < "$RETRY_FILE"
        if ! retry_plan="$(hs_retry_plan "$ROOT" "$startup_state" "$startup_state/touched-specs" < "$RETRY_FILE")"; then
            echo "Could not plan the retry; the failure record is unchanged." >&2
            echo refused > "$START_RECORD.status"
            return 1
        fi
    fi
    planned="$retry_plan"
    if [ -z "$planned" ]; then
        # A changed fingerprint may select TS after generation. Check its
        # prerequisites now, before generation can compile anything.
        if ! hs_codegen_fingerprint_matches "$ROOT"; then
            echo possible-drift > "$startup_state/generated-drift"
        fi
        planned="$(HS_INCLUDED_SUITES_INPUT="$startup_state/included-suites" select_with_includes "$startup_state/compile-inputs" "$startup_state/cli-inputs" "$startup_state/vitest-inputs" "$startup_state/generated-drift" "$tier" /dev/null "$startup_state/test-inputs" "$startup_state/crate-graph" "$startup_state/compile-owners" < "$startup_state/paths")"
        : > "$startup_state/generated-drift"
    fi
    # Filtered runs need Python only when their test filters can reach it.
    local prerequisite_suites="$planned"
    if ! {
        hs_engine_module_filters < "$startup_state/paths"
        hs_engine_filters "$startup_state/test-inputs" < "$startup_state/paths"
        for s in engine-touched engine-filtered; do
            cat "$startup_state/$s.retry" 2>/dev/null || :
        done
    } | grep -E '(^|[[:space:]])(runtime|engine|engine::tools)([[:space:]]|$)|runtime::python|engine::tools::python' >/dev/null; then
        prerequisite_suites="$(printf '%s\n' "$planned" | awk '$0 != "engine-touched" && $0 != "engine-filtered"')"
    fi
    if [ -n "$prerequisite_suites" ] && ! (cd "$ROOT" && "${HS_PREREQUISITES_CMD:-$SCRIPT_DIR/dev-prerequisites.sh}" --for "$(printf '%s\n' "$prerequisite_suites" | paste -sd , -)"); then
        startup_failed=1
        echo "Development prerequisites failed." >&2
    fi
    if ! (cd "$ROOT" && check_build_prerequisites "$planned" "$startup_state"); then
        startup_failed=1
        echo "Build prerequisites failed." >&2
    fi

    printf '%s\n' "$mode" > "$startup_state/mode"
    local checked_knowhow=""
    [ -n "${HS_GATES_CMD:-}" ] || checked_knowhow=1
    if ! STATE="$startup_state" run_independent_suites "$planned" "$checked_knowhow"; then
        independent_failed=1
        for s in $planned; do
            if [ -f "$startup_state/$s.exit" ] && [ "$(cat "$startup_state/$s.exit")" != 0 ]; then
                printf '\n%s failed during startup\n' "$s" >&2
                cat "$startup_state/$s.log" >&2
            fi
        done
    fi
    if [ "$startup_failed" = 1 ]; then
        echo refused > "$START_RECORD.status"
        echo "Startup checks failed; existing suite evidence is unchanged." >&2
        return 1
    fi

    if [ "$gated_sha" != "$(git -C "$ROOT" rev-parse HEAD)" ] ||
        [ -n "$(git -C "$ROOT" status --porcelain --untracked-files=no)" ]; then
        echo refused > "$START_RECORD.status"
        echo "The tree changed during startup checks; start again." >&2
        return 1
    fi

    # Supersede the old retry before persisting new requirements, so failed
    # startup cannot leave them attached to an older retry subset.
    if [ -n "$new_includes" ]; then rm -f "$RETRY_FILE" "$RETRY_BASE"; fi
    rm -rf "$STATE"
    mv "$startup_state" "$STATE" || return 1
    trap - EXIT
    printf '%s\n' "$gated_sha" > "$STATE/gated-sha"
    printf '%s\n' "$includes" | sed '/^$/d' > "$INCLUDE_FILE.tmp"
    mv "$INCLUDE_FILE.tmp" "$INCLUDE_FILE"
    echo "$owner" > "$STATE/pid"
    started_at "$owner" > "$STATE/pid-started"
    echo running > "$START_RECORD.status"
    echo "$mode" > "$STATE/mode"
    echo "$tier" > "$STATE/tier"
    if ! disabled="$(hs_read_check "$ROOT")"; then
        printf '%s\n' "$disabled" > "$STATE/allowlist-disabled"
        echo "Safe-path allowlist disabled for this run, so any change voids it. These files may read a safe path:"
        printf '  %s\n' "$disabled"
    fi
    hs_carried_valid "$ROOT" "$CARRY_FILE" "$STATE/compile-inputs" "$([ -f "$STATE/allowlist-disabled" ] && echo 1 || echo 0)" > "$STATE/carried"
    label="$mode"
    if [ "$tier" = fast ]; then label="$mode, fast tier"; fi

    # New requirements owe a full round: a passing retry subset cannot prove
    # that a suite added after the failed round ever ran.
    if [ -s "$RETRY_FILE" ] && [ -z "$new_includes" ]; then
        suites="$retry_plan"
        if [ -n "$suites" ]; then
            cp "$RETRY_FILE" "$STATE/retry"
            cp "$RETRY_BASE" "$STATE/full-start-sha" 2>/dev/null || :
            if [ "$independent_failed" = 1 ]; then
                printf '%s\n' "$suites" > "$STATE/suites"
                return 1
            fi
            LUCIDOS_SLOT_RECHECK_MEMORY=1 hs_generated_drift "$ROOT" > "$STATE/generated-drift"
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

    cp "$STATE/start-sha" "$STATE/full-start-sha"

    # Independent suite selection does not depend on generated drift or browser
    # specs. Record the provisional obligations even if the barrier refuses.
    early="$(select_with_includes "$STATE/compile-inputs" "$STATE/cli-inputs" "$STATE/vitest-inputs" "$STATE/generated-drift" "$tier" /dev/null "$STATE/test-inputs" "$STATE/crate-graph" "$STATE/compile-owners" < "$STATE/paths")"
    if [ "$independent_failed" = 1 ]; then
        printf '%s\n' "$early" > "$STATE/suites"
        return 1
    fi
    LUCIDOS_SLOT_RECHECK_MEMORY=1 hs_generated_drift "$ROOT" > "$STATE/generated-drift"

    # Only e2e-touched reads the specs, and it is no cargo suite, so the cargo
    # lane starts once, before the slow selection.
    early="$(select_with_includes "$STATE/compile-inputs" "$STATE/cli-inputs" "$STATE/vitest-inputs" "$STATE/generated-drift" "$tier" /dev/null "$STATE/test-inputs" "$STATE/crate-graph" "$STATE/compile-owners" < "$STATE/paths")"
    launch_suites "$(printf '%s\n' "$early" | grep -v -x driver)"
    if [ ! -f "$STATE/touched-specs" ]; then
        if ! select_touched_specs "$STATE/paths" "$STATE/touched-specs.tmp" "$STATE/e2e-touched.note"; then
            local child children
            children="$(jobs -p)"
            # Job roots belong to this shell; the recorded owner supervises it.
            stop_tree "$(for child in $children; do descendants "$child"; done)"
            wait
            echo "Could not select the touched browser specs." >&2
            return 1
        fi
        mv "$STATE/touched-specs.tmp" "$STATE/touched-specs"
    fi

    suites="$(select_with_includes "$STATE/compile-inputs" "$STATE/cli-inputs" "$STATE/vitest-inputs" "$STATE/generated-drift" "$tier" "$STATE/touched-specs" "$STATE/test-inputs" "$STATE/crate-graph" "$STATE/compile-owners" < "$STATE/paths")"
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
    )
    ) &
    local owner_job=$!
    flock_close 8
    wait "$owner_job"
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
    if startup_pending; then
        local startup_pid
        startup_pid="$(cat "$START_RECORD.pid")"
        if [ "$(started_at "$startup_pid")" = "$(cat "$START_RECORD.started")" ]; then
            stop_tree "$(descendants "$startup_pid")"
            # A replacement start may own the record by now: leave it alone.
            if [ "$(cat "$START_RECORD.pid" 2>/dev/null)" != "$startup_pid" ]; then
                echo "Stopped the startup gates."
            # The gates may have passed between the check above and the kill.
            # Then the run had begun, so this is a stopped run, not a refusal.
            elif [ "$(cat "$START_RECORD.status" 2>/dev/null)" = pending ]; then
                echo refused > "$START_RECORD.status"
                echo "Stopped the startup gates; previous suite evidence is unchanged."
            else
                mkdir -p "$STATE" && touch "$STATE/stopped"
                echo "Stopped the suite run."
            fi
            return 0
        fi
    fi
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
    cat "$STATE/suites" 2>/dev/null
    [ -f "$STATE/driver.started" ] && echo driver
}

all_done() {
    local s
    startup_pending && return 1
    runner_alive || return 0
    [ -f "$STATE/suites" ] || return 1
    for s in $(running_suites); do
        [ -f "$STATE/$s.exit" ] || return 1
    done
    return 0
}

# True once a suite has exited with a failing code. A cheap read of the exit
# files alone: suite_state still decides the verdict.
any_failed() {
    startup_pending && return 1
    startup_refused && return 0
    local s code
    for s in $(running_suites); do
        [ -f "$STATE/$s.exit" ] || continue
        code="$(cat "$STATE/$s.exit")"
        case "$code" in
            0 | 72) ;;
            "$E2E_LOCK_BUSY_EXIT") hs_is_e2e_suite "$s" || return 0 ;;
            *) return 0 ;;
        esac
    done
    return 1
}

# Sleep until every suite is done, one has failed, or the deadline passes. The
# deadline is on bash's own clock, so a slow host's checks cannot stretch it.
join_until() { # <deadline in $SECONDS>
    while ! all_done && ! any_failed && [ "$SECONDS" -lt "$1" ]; do
        sleep 1
    done
    all_done || any_failed
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
        && { ! grep -q -x engine "$STATE/suites" || [ "$(suite_state engine)" = PASS ]; }
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
    if [ ! -f "$STATE/suites" ] && ! runner_alive && ! startup_pending && ! startup_refused; then
        echo "MISSING: no suite run recorded. Start one: ./scripts/harden-suites.sh start" >&2
        return 2
    fi
    deadline=$((SECONDS + budget))
    if ! join_until "$deadline"; then
        echo "Still running after ${budget}s. Re-issue: ./scripts/harden-suites.sh wait"
        return 3
    fi
    if ! any_failed && driver_due; then
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

# The filters that matched no test in <suite>'s log: the engine-filtered
# table's, and for engine-touched the guard tests too. A module filter may
# match nothing, since not every module has tests.
engine_dead_filters() { # <suite>
    local guard=""
    [ "$1" = engine-touched ] && guard=guard_tests
    # shellcheck disable=SC2046 # one argument per filter
    hs_dead_filters "$STATE/$1.log" $guard $(hs_engine_filters "$STATE/test-inputs" < "$STATE/paths")
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
        # Only a run that could not hand off to GitHub (ADR 0584) reaches here.
        echo "SKIP every e2e slot was busy; the main check runs these specs after landing"
    elif [ "$(cat "$STATE/$suite.exit")" != 0 ]; then
        echo "FAIL see $STATE/$suite.log"
    elif { hs_is_filtered_suite "$suite" || { hs_names_engine_tests "$suite" && [ -s "$STATE/$suite.retry" ]; }; } \
        && ! grep -q -E '^running [1-9]' "$STATE/$suite.log"; then
        echo "FAIL ran no tests"
    elif { [ "$suite" = engine-filtered ] || [ "$suite" = engine-touched ]; } && [ ! -s "$STATE/$suite.retry" ] \
        && [ -n "$(engine_dead_filters "$suite")" ]; then
        echo "FAIL no test matched the filter: $(engine_dead_filters "$suite")"
    elif [ -f "$STATE/$suite.all-carried" ]; then
        echo "PASS carried from $(awk -v s="$suite" '$1 == s { print substr($3, 1, 10) }' "$STATE/carried" | sort -u | tr '\n' ' ' | sed 's/ $//')"
    else
        echo PASS
    fi
}

# A touched-spec suite that passed leaves a record the next round can carry.
record_carry() { # <suite>
    hs_record_carry "$CARRY_FILE" "$1" "$(cat "$STATE/start-sha")" "$STATE/$1.specs" "$STATE/untracked-at-start"
}

cmd_verdict() {
    if startup_pending; then
        echo "RUNNING: startup gates are still running"
        return 3
    fi
    if startup_refused; then
        echo "FAIL: startup checks refused; previous suite evidence is unchanged"
        return 1
    fi
    local s state now fail=0 rerun=0 selection_valid=1 running=""
    if [ ! -f "$STATE/suites" ]; then
        if runner_alive; then
            echo "RUNNING: the run is still setting up"
            return 3
        fi
        echo "MISSING: no suite run recorded"
        return 2
    fi
    local checked_head
    checked_head="$(git -C "$ROOT" rev-parse HEAD)"
    if [ "$checked_head" != "$(cat "$STATE/gated-sha" 2>/dev/null)" ]; then
        echo "Rechecking cheap gates for the current commit."
        if ! (cd "$ROOT" && "${HS_GATES_CMD:-$SCRIPT_DIR/harden-gates.sh}"); then
            echo "FAIL: cheap gates refused the current commit"
            return 1
        fi
        if [ "$checked_head" != "$(git -C "$ROOT" rev-parse HEAD)" ] ||
            [ -n "$(git -C "$ROOT" status --porcelain --untracked-files=no)" ]; then
            echo "RERUN: the tree changed during cheap gates"
            return 2
        fi
        printf '%s\n' "$checked_head" > "$STATE/gated-sha"
    fi
    # A fix can widen the selection, and no recorded suite would notice. A
    # retry round runs a subset by design, and the full run after it selects.
    # A recorded failure skips it: drift discovery may compile.
    if ! retry_round && ! any_failed; then
        use_main_check_setting "$STATE"
        local codegen=${HS_CODEGEN:-$SCRIPT_DIR/codegen.sh} fingerprint
        if fingerprint="$("$codegen" --fingerprint)" && [ -n "$fingerprint" ] &&
            [ "$fingerprint" = "$(cat "$STATE/codegen-fingerprint" 2>/dev/null)" ]; then
            cp "$STATE/generated-drift" "$STATE/generated-drift-now"
        else
            echo "RERUN: reselect generated outputs under the startup checks"
            return 2
        fi
        branch_paths | sort -u > "$STATE/paths-now"
        select_touched_specs "$STATE/paths-now" "$STATE/touched-specs-now" \
            || echo "(the touched specs could not be selected)" > "$STATE/touched-specs-now"
        now="$(select_with_includes "$STATE/compile-inputs" "$STATE/cli-inputs" "$STATE/vitest-inputs" "$STATE/generated-drift-now" "$(recorded_tier)" "$STATE/touched-specs-now" "$STATE/test-inputs" "$STATE/crate-graph" "$STATE/compile-owners" < "$STATE/paths-now")"
        if [ "$now" != "$(cat "$STATE/suites")" ]; then
            echo "selection VOID the diff now selects: $(printf '%s' "$now" | tr '\n' ' ')"
            rerun=1
            selection_valid=0
        fi
    fi
    : > "$STATE/failed"
    for s in $(selected_suites); do
        state="$(suite_state "$s")"
        echo "$s $state"
        case "$state" in
            PASS) ! hs_is_e2e_suite "$s" || record_carry "$s" ;;
            PASS* | SKIP*) ;;
            FAIL*) fail=1; failure_record "$s" >> "$STATE/failed" ;;
            RUNNING) running="$running $s"; echo "$s" >> "$STATE/failed" ;;
            *) rerun=1; echo "$s" >> "$STATE/failed" ;;
        esac
    done
    # Every nonterminal or invalid suite stays owed whole. Only selection
    # drift can leave requirements outside this record and invalidate the base.
    if [ "$fail" = 1 ] || [ "$rerun" = 1 ]; then
        mv "$STATE/failed" "$RETRY_FILE"
        rm -f "$RETRY_BASE"
        if [ "$selection_valid" = 1 ]; then
            cp "$STATE/full-start-sha" "$RETRY_BASE" 2>/dev/null || :
        fi
    fi
    if [ "$fail" = 1 ]; then
        echo "verdict: FAIL. Fix, commit, and return to Phase 1. The next start reruns what failed first."
        if [ -n "$running" ]; then
            echo "Still running:$running. Stop them before the next start: ./scripts/harden-suites.sh stop"
        fi
        return 1
    elif [ -n "$running" ]; then
        return 3
    elif [ "$rerun" = 1 ]; then
        echo "verdict: RERUN. Start a normal run: ./scripts/harden-suites.sh start"
        return 2
    fi
    if [ "$checked_head" != "$(git -C "$ROOT" rev-parse HEAD)" ] ||
        [ -n "$(git -C "$ROOT" status --porcelain --untracked-files=no)" ]; then
        echo "RERUN: the tree changed during verification"
        return 2
    fi
    rm -f "$RETRY_FILE" "$RETRY_BASE"
    if retry_round; then
        if hs_retry_test_only "$ROOT" "$(cat "$STATE/full-start-sha" 2>/dev/null)" > "$STATE/retry-evidence"; then
            cat "$STATE/retry-evidence"
            echo "verdict: PASS. What failed now passes; only tests changed since the last full round."
            return 0
        fi
        cat "$STATE/retry-evidence"
        echo "verdict: RERUN. What failed now passes; non-test or unverifiable changes require the full set: ./scripts/harden-suites.sh start"
        return 2
    fi
    echo "verdict: PASS"
    return 0
}

# The specs a touched-spec suite ran.
suite_specs() { # <suite>
    cat "$STATE/$1.specs"
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

# One step's queued, start, end and exit code as a JSON object, with null for
# a field it never reached.
step_timings() { # <name>
    local queued start end rc
    queued="$(cat "$STATE/$1.queued" 2>/dev/null)"
    start="$(cat "$STATE/$1.start" 2>/dev/null)"
    end="$(cat "$STATE/$1.end" 2>/dev/null)"
    rc="$(cat "$STATE/$1.exit" 2>/dev/null)"
    printf '{"queued": %s, "start": %s, "end": %s, "exit": %s}' \
        "${queued:-null}" "${start:-null}" "${end:-null}" "${rc:-null}"
}

# Each suite's timings, and the codegen step's when it ran, as one JSON object
# for the HardenRunCompleted payload. Queued to start is the wait for the lane
# or the codegen step.
cmd_timings() {
    local s first=1 round=full codegen=null
    if startup_pending || startup_refused || [ ! -f "$STATE/suites" ]; then
        echo '{}'
        return 0
    fi
    retry_round && round=retry
    [ ! -f "$STATE/codegen-step.queued" ] || codegen="$(step_timings codegen-step)"
    printf '{"round": "%s", "mode": "%s", "codegen_step": %s, "suites": {' "$round" "$(cat "$STATE/mode")" "$codegen"
    for s in $(running_suites); do
        [ "$first" = 1 ] || printf ', '
        first=0
        printf '"%s": %s' "$s" "$(step_timings "$s")"
    done
    echo '}}'
}

case "${1:-}" in
    start) shift; cmd_start "$@" ;;
    stop) cmd_stop ;;
    wait) shift; cmd_wait "$@" ;;
    run-driver) date +%s > "$STATE/driver.queued"; run_suite driver ;;
    verdict) cmd_verdict ;;
    timings) cmd_timings ;;
    -h | --help) usage ;;
    *) usage >&2; exit 1 ;;
esac
