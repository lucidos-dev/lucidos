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
#   --ios            Launch iOS Simulator with Safari (requires Xcode)
#   --               Everything after this is passed to Playwright
#
# Examples:
#   ./scripts/e2e-browser.sh                           # All tests
#   ./scripts/e2e-browser.sh -h -f chat.spec.ts        # Headed, single file
#   ./scripts/e2e-browser.sh -- --grep "sends message" # Filter by test name
#   ./scripts/e2e-browser.sh --webkit                  # WebKit mobile tests
#   ./scripts/e2e-browser.sh --ios                     # iOS Simulator
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"

source "$SCRIPT_DIR/lib/e2e.sh"

HEADED=""
TEST_FILE=""
NO_RESET=""
USE_WEBKIT=""
USE_IOS=""
IOS_ARGS=()
PW_ARGS=()

while [[ $# -gt 0 ]]; do
    case "$1" in
        -h|--headed) HEADED=1; shift ;;
        -f) TEST_FILE="$2"; shift 2 ;;
        --no-reset) NO_RESET=1; shift ;;
        --webkit) USE_WEBKIT=1; shift ;;
        --ios) USE_IOS=1; shift ;;
        --device) IOS_ARGS+=(--device "$2"); shift 2 ;;
        --screenshot) IOS_ARGS+=(--screenshot); shift ;;
        --pwa) IOS_ARGS+=(--pwa); shift ;;
        --) shift; PW_ARGS+=("$@"); break ;;
        *) PW_ARGS+=("$1"); shift ;;
    esac
done

# iOS Simulator mode — delegate to e2e-ios.sh
if [ -n "$USE_IOS" ]; then
    exec "$SCRIPT_DIR/e2e-ios.sh" "${IOS_ARGS[@]}"
fi

setup_e2e_session e2e-browser --cleanup-worktrees-on-teardown

echo "Running browser e2e tests (port $VITE_PORT)"

cd "$PROJECT_DIR/crates/lucidos-app"

npm install

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
# stop_webkit_reaper) already owns the EXIT trap. Under the umbrella ($LUCIDOS_E2E_UMBRELLA)
# setup_e2e_session installs no trap, so register one here that stops the reaper
# when this browser phase exits — before the umbrella's wasm/embedder phases.
start_webkit_reaper
if [ -n "${LUCIDOS_E2E_UMBRELLA:-}" ]; then
    trap stop_webkit_reaper EXIT
fi

CMD=(npx playwright test)
[ -n "$TEST_FILE" ] && CMD+=("$TEST_FILE")
[ ${#PW_ARGS[@]} -gt 0 ] && CMD+=("${PW_ARGS[@]}")

# Detect whether the caller already pinned a project (via --webkit or `-- --project=`).
# If so, run once. Otherwise, loop through every project with a clean DB between
# each — the workspace DB is not isolated across projects.
USER_PINNED_PROJECT=""
[ -n "$USE_WEBKIT" ] && USER_PINNED_PROJECT=1
for arg in "${PW_ARGS[@]:-}"; do
    case "$arg" in --project=*|--project) USER_PINNED_PROJECT=1 ;; esac
done

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
run_browser_project() {
    local project="$1"
    local rc=0
    local f base
    local cc_specs=()
    local nav_specs=()
    # Only phase-split the FULL mobile-webkit run. When the caller pinned a
    # spec/file filter (-f <file>, a positional, or -- args), honor it verbatim:
    # appending the whole spec list would OR the filter away (Playwright unions
    # positional filters), running the entire suite instead of the requested
    # subset. Targeted runs fall through to the single-pass call below.
    if [ "$project" = "mobile-webkit" ] && [ -z "$TEST_FILE" ] && [ "${#PW_ARGS[@]}" -eq 0 ]; then
        for f in e2e/*.spec.ts; do
            [ -e "$f" ] || continue
            base="$(basename "$f")"
            if grep -q "pickComposeDestination" "$f" 2>/dev/null; then
                cc_specs+=("$base")
            else
                nav_specs+=("$base")
            fi
        done
        if [ "${#nav_specs[@]}" -gt 0 ] && [ "${#cc_specs[@]}" -gt 0 ]; then
            echo "── mobile-webkit phase 1/2: ${#nav_specs[@]} navigation specs (quiet engine) ──"
            "${CMD[@]}" --project="$project" "${nav_specs[@]}" || rc=$?
            echo "── mobile-webkit phase 2/2: ${#cc_specs[@]} CC-subprocess specs ──"
            "${CMD[@]}" --project="$project" "${cc_specs[@]}" || rc=$?
            return "$rc"
        fi
    fi
    "${CMD[@]}" --project="$project" || rc=$?
    return "$rc"
}

if [ -n "$USER_PINNED_PROJECT" ]; then
    [ -n "$USE_WEBKIT" ] && CMD+=(--project=mobile-webkit)
    "${CMD[@]}"
else
    # Run every project even if an earlier one failed, so the user sees all
    # results in one run. Aggregate exit status so the script still exits
    # non-zero when any project failed. macOS ships bash 3.x — no associative
    # arrays, so use parallel indexed arrays.
    #
    # mobile-webkit runs FIRST so its contention-sensitive WebContent spawns
    # happen before chromium/mobile add two more passes of CC-subprocess churn to
    # the host. A DB reset runs before each *subsequent* project (the workspace DB
    # isn't isolated across projects); mobile-webkit gets the freshly-booted state.
    PROJECTS=(mobile-webkit chromium mobile)
    PROJECT_RCS=()
    overall_rc=0
    for i in "${!PROJECTS[@]}"; do
        project="${PROJECTS[$i]}"
        if [ "$i" -gt 0 ] && [ -z "$NO_RESET" ]; then
            echo ""
            echo "── Resetting DB before project: $project ──"
            reset_e2e_database
        fi
        echo ""
        echo "── Running project: $project ──"
        rc=0
        run_browser_project "$project" || rc=$?
        PROJECT_RCS[$i]=$rc
        [ "$rc" -ne 0 ] && overall_rc=$rc
    done
    echo ""
    echo "── Per-project exit codes ──"
    for i in "${!PROJECTS[@]}"; do
        echo "  ${PROJECTS[$i]}: ${PROJECT_RCS[$i]}"
    done
    exit "$overall_rc"
fi
