#!/bin/bash
# Run Playwright browser e2e tests against the e2e-test workspace.
#
# Usage:
#   ./scripts/e2e-browser.sh [options] [-- playwright args]
#
# Options:
#   -h, --headed     Run with visible browser
#   -f <file>        Run specific test file (e.g., chat.spec.ts)
#   --no-reset       Skip DB reset AND leave the workspace running for the next
#                    invocation. Use for fast iteration on a single spec.
#   --webkit         Run mobile tests on WebKit (iOS Safari engine)
#   --no-webkit      Run every browser project EXCEPT mobile-webkit
#   --ios            Launch iOS Simulator with Safari (requires Xcode)
#   --               Everything after this is passed to Playwright
#
# Examples:
#   ./scripts/e2e-browser.sh                           # All tests
#   ./scripts/e2e-browser.sh -h -f chat.spec.ts        # Headed, single file
#   ./scripts/e2e-browser.sh -- --grep "sends message" # Filter by test name
#   ./scripts/e2e-browser.sh --webkit                  # WebKit mobile tests
#   ./scripts/e2e-browser.sh --no-webkit               # The cheap projects only
#   ./scripts/e2e-browser.sh --ios                     # iOS Simulator
#
# mobile-webkit costs about 15 GB of macOS VM compressor and the other five
# projects cost about 0.6 GB between them, so the two are run separately:
# `--no-webkit` here (or on scripts/e2e.sh) for the cheap set, then `--webkit`
# on a cold host for the expensive one. See docs/e2e-test-decisions.md.
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"

source "$SCRIPT_DIR/lib/e2e.sh"

HEADED=""
TEST_FILE=""
NO_RESET=""
USE_WEBKIT=""
SKIP_WEBKIT=""
USE_IOS=""
IOS_ARGS=()
PW_ARGS=()

while [[ $# -gt 0 ]]; do
    case "$1" in
        -h|--headed) HEADED=1; shift ;;
        -f) TEST_FILE="$2"; shift 2 ;;
        --no-reset) NO_RESET=1; shift ;;
        --webkit) USE_WEBKIT=1; shift ;;
        --no-webkit) SKIP_WEBKIT=1; shift ;;
        --ios) USE_IOS=1; shift ;;
        --device) IOS_ARGS+=(--device "$2"); shift 2 ;;
        --screenshot) IOS_ARGS+=(--screenshot); shift ;;
        --pwa) IOS_ARGS+=(--pwa); shift ;;
        --) shift; PW_ARGS+=("$@"); break ;;
        *) PW_ARGS+=("$1"); shift ;;
    esac
done

if [ -n "$USE_WEBKIT" ] && [ -n "$SKIP_WEBKIT" ]; then
    echo "e2e-browser.sh: --webkit and --no-webkit contradict each other" >&2
    exit 1
fi

# iOS Simulator mode — delegate to e2e-ios.sh
if [ -n "$USE_IOS" ]; then
    exec "$SCRIPT_DIR/e2e-ios.sh" "${IOS_ARGS[@]}"
fi

setup_e2e_session e2e-browser --cleanup-worktrees-on-teardown

# Host-load backpressure guard — the e2e lock is now held (by setup_e2e_session in
# standalone mode, or by the umbrella scripts/e2e.sh under $LUCIDOS_E2E_UMBRELLA),
# so this is the chokepoint right before the Playwright browser swarm spawns, for
# BOTH entry paths. If the host is already saturated it waits/backs off; if it
# stays saturated past the wait cap it returns HOST_LOAD_SATURATED_EXIT (75) and we
# exit cleanly rather than piling the swarm onto a pegged host and wedging the
# machine (2026-07-01 incident — see docs/e2e-test-decisions.md + host_load_guard.sh).
# The lock is released on the way out by the EXIT-trap chain (standalone: this
# script's teardown_e2e; umbrella: e2e.sh's set -e + teardown_e2e), so exit 75 never
# leaves a stale lock. Deliberately invoked ONCE here, not also in e2e.sh, so the
# umbrella run doesn't double the wait. e2e-api.sh is intentionally not guarded.
wait_for_host_load || exit $?

echo "Running browser e2e tests (port $VITE_PORT)"

cd "$PROJECT_DIR/crates/lucidos-app"

# Strict, deterministic install from the committed root lockfile (npm workspaces
# hoists to the root, so run ci there; subshell keeps cwd at the app dir for
# Playwright below).
( cd "$PROJECT_DIR" && npm ci )

export E2E_WORKSPACE
[ -n "$HEADED" ] && export HEADED=1

# Start the WebKit RSS reaper — a host-memory safety net for the mobile-webkit
# browser-process wedge (see docs/e2e-test-decisions.md). A wedged WebContent
# child sits on its RSS; under nightly load several pile up and exhaust host
# memory. The reaper SIGKILLs any single Playwright WebKit child over the cap;
# Playwright's retries:1 recovers the affected test. Additive to gotoWithRetry +
# retries:1, not a replacement.
#
# Teardown: in standalone mode setup_e2e_session's teardown_e2e (which calls
# stop_e2e_background_guards) already owns the EXIT trap. Under the umbrella
# ($LUCIDOS_E2E_UMBRELLA) setup_e2e_session installs no trap, so register one here
# that stops both run-scoped loops — the reaper and the host-load sampler below —
# when this browser phase exits, before the umbrella's wasm/embedder phases.
start_webkit_reaper

# Mid-run host-load sampler. wait_for_host_load above only knows about the
# instant it fired; during the 2026-07-26 nightly an external daemon burst pinned
# the host at load 83-227 for ~40 minutes AFTER that gate passed, starving the
# browsers into timeouts that read exactly like product failures. The sampler
# records load throughout the run so `finish` can classify such a run instead of
# letting it pass for a product verdict. It never retries and never alters the
# exit code (see report_host_load_saturation).
start_host_load_sampler

if [ -n "${LUCIDOS_E2E_UMBRELLA:-}" ]; then
    trap stop_e2e_background_guards EXIT
fi

# Every exit path funnels through here so the sampler is drained and the run is
# classified exactly once, whichever branch below ran.
finish() {
    local rc="$1"
    stop_host_load_sampler
    report_host_load_saturation "$rc"
    report_memory_stop
    report_webkit_excluded "$SKIP_WEBKIT"
    exit "$rc"
}

CMD=(npx playwright test)
[ -n "$TEST_FILE" ] && CMD+=("$TEST_FILE")
[ ${#PW_ARGS[@]} -gt 0 ] && CMD+=("${PW_ARGS[@]}")

# Detect whether the caller already pinned a project (via --webkit or `-- --project=`).
# If so, run once. Otherwise, loop through every project with a clean DB between
# each — the workspace DB is not isolated across projects. The same pass notes a
# caller-pinned --output so set_output_dir never silently overrides it.
USER_PINNED_PROJECT=""
USER_PINNED_OUTPUT=""
[ -n "$USE_WEBKIT" ] && USER_PINNED_PROJECT=1
for arg in "${PW_ARGS[@]:-}"; do
    case "$arg" in
        --project=*|--project) USER_PINNED_PROJECT=1 ;;
        --output=*|--output) USER_PINNED_OUTPUT=1 ;;
    esac
done

# Failure-trace retention across a whole run. Playwright DELETES its output dir at
# the START of every `playwright test` invocation (createRemoveOutputDirsTask), and
# the default is the entire `test-results/` tree — but one suite run makes MANY
# invocations: one per project, plus one per mobile-webkit chunk. On the default
# each pass therefore erased the previous pass's retained traces + screenshots and
# only the LAST project's survived, so an unattended nightly failure left nothing to
# triage with. Fix: wipe ONE root here, then give every invocation its own subdir
# under it (set_output_dir) — nothing is wiped mid-run. (These are Playwright's
# "output artifacts", NOT Lucidos *artifacts* — they're ephemeral, gitignored test
# output, so the naming here stays on `output` to keep the glossary term clean.)
if [ -n "$TEST_FILE" ] || [ "${#PW_ARGS[@]}" -gt 0 ]; then
    # Targeted repro: clear only its own corner, so a preceding full run's evidence
    # — usually the very thing you're reproducing against — stays intact.
    PW_OUTPUT_ROOT="test-results/targeted"
    rm -rf "$PW_OUTPUT_ROOT"
else
    # Full run: clean slate for the whole tree, which also clears any leftover
    # targeted dirs and the earlier flat layout.
    PW_OUTPUT_ROOT="test-results/full"
    rm -rf test-results
fi

# Per-invocation --output, kept as an array so "pinned by the caller" passes no
# argument at all rather than an empty one.
OUTPUT_ARG=()
set_output_dir() {
    OUTPUT_ARG=()
    [ -n "$USER_PINNED_OUTPUT" ] || OUTPUT_ARG=(--output="$PW_OUTPUT_ROOT/$1")
}

# ── Host memory ceiling between mobile-webkit chunks ──────────────────
# Chunking resets WebContent accumulation. The cost it leaves behind lands in the
# macOS VM compressor, which nothing else in this harness reads. On a cold host
# mobile-webkit drove the compressor from 12 GB to 17 GB in ten minutes, with
# unused memory down to 78 MB. The WebKit reaper fired zero times through that
# climb. Many short-lived WebContent processes carry the cost between them, so no
# single RSS approaches the reaper's per-process cap.
#
# `kern.memorystatus_vm_pressure_level` is no use as a stop condition either. It
# stayed at "normal" for the whole climb. The compressor is the one number that
# moves in time to act on.
#
# The ceiling defaults to 12 GB. LUCIDOS_E2E_COMPRESSOR_MAX_GB overrides it, the
# same shape as LUCIDOS_E2E_WEBKIT_CHUNK.
#
# A memory stop must never read as a red project, so it carries its own exit
# code. 71 is sysexits' EX_OSERR: an OS resource condition, not a test verdict.
# It sits beside the host-load guard's 75 (EX_TEMPFAIL). Playwright exits 0, 1 or
# 130, so 71 can never collide with a Playwright code.
HOST_MEMORY_STOP_EXIT=71

# Fold one phase or chunk exit code into an aggregate, and echo the winner. A
# memory stop is the WEAKEST non-zero code: it says the run was cut short, never
# that the product is broken. So a real test failure always outranks it, from
# whichever phase or chunk it came. Stated once here because the run aggregates
# exit codes at three levels. A stop that overwrote a failure at any of them
# would hide a red run behind a host-memory verdict.
merge_rc() {
    local current="$1" incoming="$2"
    if [ "$incoming" -eq 0 ]; then
        echo "$current"
    elif [ "$current" -eq 0 ] || [ "$current" -eq "$HOST_MEMORY_STOP_EXIT" ]; then
        echo "$incoming"
    elif [ "$incoming" -eq "$HOST_MEMORY_STOP_EXIT" ]; then
        echo "$current"
    else
        echo "$incoming"
    fi
}

# Set to the project name at the boundary that tripped the ceiling. The phase
# split and the project loop both read it. Neither starts more work on a host we
# have already refused to load further.
MEMORY_STOPPED=""
MEMORY_STOP_DETAIL=""

# Compressor size in GB, from vm_stat. The page size comes from vm_stat's own
# header rather than a constant, because it is 16 KB on Apple silicon and 4 KB on
# Intel. Echoes nothing when it cannot read; the caller then fails open, the same
# posture as the host-load guard and the reaper. A guard that cannot measure must
# never stop the suite.
read_compressor_gb() {
    vm_stat 2>/dev/null | awk '
        /page size of/ { for (i = 1; i < NF; i++) if ($i == "of") page = $(i + 1) + 0 }
        /^Pages occupied by compressor:/ { pages = $NF + 0 }
        END { if (page > 0 && pages > 0) printf "%.2f", pages * page / 1073741824 }
    '
}

# Report the compressor at one chunk boundary. Returns non-zero when it is over
# the ceiling, and records the reason for the final verdict. Every boundary
# prints a line, so an unattended run leaves the whole curve in its log instead
# of only the point where it stopped.
check_compressor_at_boundary() {
    local where="$1"
    local max="${LUCIDOS_E2E_COMPRESSOR_MAX_GB:-12}"
    # An override that is not a positive number falls back to the default. A
    # garbage ceiling would otherwise stop the suite at the first boundary.
    awk -v m="$max" 'BEGIN { exit (m + 0 > 0) ? 0 : 1 }' || max=12

    local gb over
    gb="$(read_compressor_gb)"
    if [ -z "$gb" ]; then
        echo "[e2e-mem] after $where: compressor unreadable, ceiling check skipped"
        return 0
    fi
    echo "[e2e-mem] after $where: compressor $gb GB"

    over="$(awk -v g="$gb" -v m="$max" 'BEGIN { print (g > m) ? 1 : 0 }')"
    [ "$over" = 1 ] || return 0

    MEMORY_STOP_DETAIL="At $where the compressor was $gb GB, over the $max GB ceiling."
    echo ""
    echo "[e2e-mem] STOP: compressor $gb GB is over the $max GB ceiling."
    echo "[e2e-mem] Stopping here rather than pushing the host further."
    return 1
}

# Print the compressor without judging it, once, before any browser work.
# mobile-webkit needs roughly 15 GB to finish, so what the phase STARTED from
# decides how far it gets, and nothing else in the log records that number.
report_compressor_start() {
    local gb
    gb="$(read_compressor_gb)"
    if [ -z "$gb" ]; then
        echo "[e2e-mem] browser phase start: compressor unreadable"
    else
        echo "[e2e-mem] browser phase start: compressor $gb GB"
    fi
}

# Final verdict for a run the ceiling cut short. finish calls it, so every exit
# path says this once, last, where an unattended reader looks.
report_memory_stop() {
    [ -n "$MEMORY_STOPPED" ] || return 0
    echo ""
    echo "[e2e-mem] STOPPED ON HOST MEMORY during $MEMORY_STOPPED."
    echo "[e2e-mem] $MEMORY_STOP_DETAIL"
    echo "[e2e-mem] Coverage is incomplete: work after that point did not run."
    echo "[e2e-mem] Exit $HOST_MEMORY_STOP_EXIT marks a memory stop, never a failing test."
    echo "[e2e-mem] Compare this against the browser phase start line above."
    echo "[e2e-mem] A smaller LUCIDOS_E2E_WEBKIT_CHUNK cannot buy headroom that"
    echo "[e2e-mem] was never there. mobile-webkit needs a cold host: rerun it"
    echo "[e2e-mem] alone with ./scripts/e2e-browser.sh --webkit."
    echo "[e2e-mem] LUCIDOS_E2E_COMPRESSOR_MAX_GB raises the ceiling if you must."
}

# Run a browser project. For mobile-webkit, split the run into two ordered
# phases — navigation/UI specs (no Claude Code subprocess spawns) FIRST, then the
# CC-subprocess-spawning specs. This shrinks the contention window behind the
# mobile-webkit nav-wedge's RESIDUAL variant (a WebContent cold-start/document-load
# stall under heavy host load — see docs/e2e-test-decisions.md). The wedge's
# PRIMARY variant (WebKit macOS system-proxy/PAC discovery on the first navigation
# of each fresh context) is fixed at the source by the explicit `proxy` on the
# mobile-webkit project in playwright.config.ts. Keeping nav-sensitive specs out of
# the CC-spawn window is recovery-frequency reduction for the residual variant, not
# a cure, and is harmless to keep. CC specs are auto-detected by helper usage
# (pickComposeDestination — the compose destination picker is the entry point
# for spawning a coding-agent thread) so newly added specs classify themselves;
# if the set can't be split we fall back to a single run. Other projects always
# run in one pass.
# Run a list of spec files through CMD in fresh-process chunks of CHUNK_SIZE files
# each. Each `npx playwright test` invocation launches a fresh browser, so
# WebKit's per-context WebContent memory accumulation RESETS between chunks —
# keeping host pressure below the threshold that makes the first navigation of a
# fresh page cold-start-stall (the mobile-webkit nav-wedge, reproduced at
# retries:0; root cause + rationale in
# docs/plans/2026-06-27-mobile-webkit-shard-contention.md). Coverage is identical
# to one big pass — only the process boundaries change. Exit codes aggregate, so
# any failed chunk fails the whole. Chunk size is overridable via
# LUCIDOS_E2E_WEBKIT_CHUNK for tuning without a code change.
#
# The default is 3 specs per chunk. At 8 the compressor still climbed 5 GB inside
# this project. The nightly died here twice, before the wasm and embedder
# projects ever started. More boundaries is the only lever this loop has on that
# curve. Each boundary BETWEEN chunks also checks the compressor (see
# check_compressor_at_boundary) and stops the loop when the host is over the
# ceiling. The boundary after the last chunk is the caller's: only it knows
# whether another phase or another project follows, and a stop with nothing
# left to stop would report a finished run as a cut-short one.
run_specs_chunked() {
    local project="$1"; shift
    local label="$1"; shift
    local specs=("$@")
    local total="${#specs[@]}"
    local size="${LUCIDOS_E2E_WEBKIT_CHUNK:-3}"
    local rc=0 start=0 chunk_no=0 nchunks
    nchunks=$(( (total + size - 1) / size ))
    while [ "$start" -lt "$total" ]; do
        chunk_no=$(( chunk_no + 1 ))
        local chunk=("${specs[@]:start:size}")
        echo "── mobile-webkit $label chunk $chunk_no/$nchunks: ${#chunk[@]} specs (fresh browser) ──"
        # Own output dir per chunk (see set_output_dir) — otherwise each chunk
        # would erase the previous chunk's failure traces/screenshots.
        set_output_dir "$project-$label-$chunk_no"
        "${CMD[@]}" --project="$project" "${OUTPUT_ARG[@]}" "${chunk[@]}" || rc=$?
        start=$(( start + size ))
        # BETWEEN chunks only. The boundary after the LAST one belongs to the
        # caller, which is the only code that knows whether another phase or
        # another project follows it. A stop needs something left to stop: with
        # mobile-webkit running last, an unconditional check here would report a
        # run that finished everything as a run that was cut short.
        [ "$start" -lt "$total" ] || break
        if ! check_compressor_at_boundary "$project $label chunk $chunk_no/$nchunks"; then
            MEMORY_STOPPED="$project"
            # MEMORY_STOPPED carries the stop on its own, so merge_rc can keep a
            # failing chunk's code and neither signal hides the other.
            rc="$(merge_rc "$rc" "$HOST_MEMORY_STOP_EXIT")"
            break
        fi
    done
    return "$rc"
}

run_browser_project() {
    local project="$1"
    local rc=0
    local f base
    local cc_specs=()
    local nav_specs=()
    # Only shard the FULL mobile-webkit run. When the caller pinned a spec/file
    # filter (-f <file>, a positional, or -- args), honor it verbatim: appending
    # the whole spec list would OR the filter away (Playwright unions positional
    # filters), running the entire suite instead of the requested subset. Targeted
    # runs fall through to the single-pass call below.
    if [ "$project" = "mobile-webkit" ] && [ -z "$TEST_FILE" ] && [ "${#PW_ARGS[@]}" -eq 0 ]; then
        for f in e2e/*.spec.ts; do
            [ -e "$f" ] || continue
            base="$(basename "$f")"
            # Skip *-desktop.spec.ts: the mobile-webkit project testIgnores them
            # (playwright.config.ts), so they run zero tests here. Including them
            # was harmless in the single-pass run, but a SHARD landing entirely on
            # ignored files would make `playwright test` exit "no tests found" (rc 1)
            # and fail the chunk spuriously. Excluding them keeps every chunk real.
            case "$base" in *-desktop.spec.ts) continue ;; esac
            if grep -q "pickComposeDestination" "$f" 2>/dev/null; then
                cc_specs+=("$base")
            else
                nav_specs+=("$base")
            fi
        done
        if [ "${#nav_specs[@]}" -gt 0 ] && [ "${#cc_specs[@]}" -gt 0 ]; then
            # Nav specs first (quiet engine), then CC-subprocess specs — each phase
            # sharded into fresh-process chunks so WebKit memory can't accumulate
            # across the whole suite into the cold-start-stall zone.
            local nav_rc=0 cc_rc=0
            echo "── mobile-webkit phase 1/2: ${#nav_specs[@]} navigation specs (sharded) ──"
            run_specs_chunked "$project" "nav" "${nav_specs[@]}" || nav_rc=$?
            # The nav/CC boundary, which the chunk loop deliberately leaves to
            # its caller. Phase 2 is what follows, so there is real work to stop.
            if [ -z "$MEMORY_STOPPED" ] \
                && ! check_compressor_at_boundary "$project phase 1/2 (nav)"; then
                MEMORY_STOPPED="$project"
                nav_rc="$(merge_rc "$nav_rc" "$HOST_MEMORY_STOP_EXIT")"
            fi
            if [ -n "$MEMORY_STOPPED" ]; then
                # Phase 2 is the heavier half. Starting it on a host already over
                # the ceiling is exactly what the ceiling exists to prevent.
                echo "── mobile-webkit phase 2/2 SKIPPED: stopped on host memory ──"
                return "$nav_rc"
            fi
            echo "── mobile-webkit phase 2/2: ${#cc_specs[@]} CC-subprocess specs (sharded) ──"
            run_specs_chunked "$project" "CC" "${cc_specs[@]}" || cc_rc=$?
            # A CC-phase stop must not overwrite a failing nav phase (see
            # merge_rc), which plain last-wins aggregation would do.
            rc="$(merge_rc "$rc" "$nav_rc")"
            rc="$(merge_rc "$rc" "$cc_rc")"
            return "$rc"
        fi
    fi
    # Own output dir per project (see set_output_dir) — otherwise the NEXT
    # project's invocation would wipe this one's, chunk dirs included.
    set_output_dir "$project"
    "${CMD[@]}" --project="$project" "${OUTPUT_ARG[@]}" || rc=$?
    return "$rc"
}

if [ -n "$USE_WEBKIT" ] && [ -z "$TEST_FILE" ] && [ "${#PW_ARGS[@]}" -eq 0 ]; then
    # `--webkit` with no filter: route through run_browser_project so a manual
    # webkit run gets the SAME sharding/phase-split as the nightly full run (and so
    # the sharding is validatable in isolation). A filtered `--webkit -f X` (or any
    # `--`/positional arg) still falls through to the single-pass branch below —
    # run_browser_project's own guard would single-pass it anyway, but keeping it
    # here avoids appending --project twice.
    # This is the documented "run it alone, on a cold host" recipe, so it is the
    # run whose STARTING compressor matters most: it decides how far the project
    # gets, and no other line records it.
    echo ""
    report_compressor_start
    webkit_rc=0
    run_browser_project mobile-webkit || webkit_rc=$?
    finish "$webkit_rc"
elif [ -n "$USER_PINNED_PROJECT" ]; then
    [ -n "$USE_WEBKIT" ] && CMD+=(--project=mobile-webkit)
    set_output_dir pinned
    # Capture rather than letting `set -e` exit here, so a failing pinned run
    # still gets drained + classified by finish.
    pinned_rc=0
    "${CMD[@]}" "${OUTPUT_ARG[@]}" || pinned_rc=$?
    finish "$pinned_rc"
else
    # Run every project even if an earlier one failed, so the user sees all
    # results in one run. Aggregate exit status so the script still exits
    # non-zero when any project failed. macOS ships bash 3.x — no associative
    # arrays, so use parallel indexed arrays.
    #
    # mobile-webkit runs LAST, and this reversed a deliberate earlier ordering.
    # It used to run first, to keep its contention-sensitive WebContent spawns
    # ahead of two more passes of CC-subprocess churn. The wedge that argued for
    # is fixed at the source (the explicit `proxy` on the mobile-webkit project
    # in playwright.config.ts) and the projects run sequentially, so the churn
    # was never concurrent with it anyway. What is not fixed is the memory: this
    # one project costs about 15 GB of compressor and the other two cost about
    # 0.6 GB between them. Whatever is queued behind it is what a memory stop
    # loses, so nothing is.
    #
    # A DB reset runs before each *subsequent* project (the workspace DB isn't
    # isolated across projects); the first gets the freshly-booted state. Each
    # reset recreates the database and restarts the engine on it, on the same
    # binary, since build_e2e_engine_once never recompiles mid-suite. So every
    # project sees a brand-new workspace database, seeds included.
    PROJECTS=(chromium mobile mobile-webkit)
    # --no-webkit leaves the expensive project for its own run. Dropped from the
    # list rather than skipped inside it, so the per-project table below reports
    # what actually ran; report_webkit_excluded says what did not.
    [ -n "$SKIP_WEBKIT" ] && PROJECTS=(chromium mobile)
    PROJECT_RCS=()
    overall_rc=0
    echo ""
    report_compressor_start
    for i in "${!PROJECTS[@]}"; do
        project="${PROJECTS[$i]}"
        if [ -n "$MEMORY_STOPPED" ]; then
            # The host is over the compressor ceiling, so the projects after the
            # stop do not run. Record the stop code rather than leaving a hole.
            # A project with no rc reads as a harness bug in the table below. An
            # rc of 0 would read as green work that never ran.
            echo ""
            echo "── Skipping project (stopped on host memory): $project ──"
            PROJECT_RCS+=("$HOST_MEMORY_STOP_EXIT")
            continue
        fi
        if [ "$i" -gt 0 ] && [ -z "$NO_RESET" ]; then
            echo ""
            echo "── Resetting DB before project: $project ──"
            reset_e2e_database
        fi
        echo ""
        echo "── Running project: $project ──"
        rc=0
        run_browser_project "$project" || rc=$?
        # APPEND — never PROJECT_RCS[i]=. The body above calls into the e2e lib,
        # and any lib function that leaks a loop variable named `i` (that was the
        # 2026-07-26 bug: ensure_workspace_running's readiness counter) would make
        # an indexed write land in the wrong slot, leaving a hole at the last
        # project. Appending in lockstep with PROJECTS can't produce a hole.
        PROJECT_RCS+=("$rc")
        # merge_rc, not last-wins: a later project's memory stop must not
        # overwrite an earlier project's real failure.
        overall_rc="$(merge_rc "$overall_rc" "$rc")"
        # The chunk loop guards mobile-webkit only. A project boundary is the
        # same hazard: what comes next is another browser swarm, on whatever the
        # last one left behind. Skipped after the LAST project, where nothing
        # follows: a stop needs something left to stop. This project's own rc
        # stays untouched either way, because it finished.
        if [ -z "$MEMORY_STOPPED" ] && [ "$i" -lt "$(( ${#PROJECTS[@]} - 1 ))" ] \
            && ! check_compressor_at_boundary "the boundary after project $project"; then
            MEMORY_STOPPED="$project"
            overall_rc="$(merge_rc "$overall_rc" "$HOST_MEMORY_STOP_EXIT")"
        fi
    done

    # Pair each project with its recorded rc; a short array (or a stray empty
    # entry) surfaces as UNKNOWN and forces a non-zero exit inside the reporter.
    PROJECT_ENTRIES=()
    for i in "${!PROJECTS[@]}"; do
        PROJECT_ENTRIES+=("${PROJECTS[$i]}:${PROJECT_RCS[$i]:-}")
    done
    final_rc=0
    report_project_exit_codes "$overall_rc" "${PROJECT_ENTRIES[@]}" || final_rc=$?
    finish "$final_rc"
fi
