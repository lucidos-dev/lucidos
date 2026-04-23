#!/bin/bash
# Run Playwright browser e2e tests against the e2e-test workspace.
#
# Usage:
#   ./scripts/e2e-browser.sh [options] [-- playwright args]
#
# Options:
#   -h, --headed     Run with visible browser
#   -f <file>        Run specific test file (e.g., chat.spec.ts)
#   --no-reset       Skip database reset
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

acquire_e2e_lock e2e-browser || exit 1
ensure_workspace_running
teardown_e2e() {
    [ -z "$NO_RESET" ] && cleanup_e2e_worktrees
    stop_e2e_workspace
    release_e2e_lock
}
trap teardown_e2e EXIT
trap 'exit 130' INT TERM
[ -z "$NO_RESET" ] && cleanup_e2e_worktrees
[ -z "$NO_RESET" ] && reset_e2e_database

echo "Running browser e2e tests (port $VITE_PORT)"

cd "$PROJECT_DIR/crates/cognos-app"

npm install

export E2E_WORKSPACE
[ -n "$HEADED" ] && export HEADED=1

CMD=(npx playwright test)
[ -n "$TEST_FILE" ] && CMD+=("$TEST_FILE")
[ ${#PW_ARGS[@]} -gt 0 ] && CMD+=("${PW_ARGS[@]}")

# Detect whether the caller already pinned a project (via --webkit or `-- --project=`).
# If so, run once. Otherwise, loop through every project with a clean DB between
# each — the workspace DB is not isolated across projects, and the last project
# (mobile-webkit) was timing out from accumulated event-store state.
USER_PINNED_PROJECT=""
[ -n "$USE_WEBKIT" ] && USER_PINNED_PROJECT=1
for arg in "${PW_ARGS[@]:-}"; do
    case "$arg" in --project=*|--project) USER_PINNED_PROJECT=1 ;; esac
done

if [ -n "$USER_PINNED_PROJECT" ]; then
    [ -n "$USE_WEBKIT" ] && CMD+=(--project=mobile-webkit)
    "${CMD[@]}"
else
    PROJECTS=(chromium mobile mobile-webkit)
    for i in "${!PROJECTS[@]}"; do
        project="${PROJECTS[$i]}"
        if [ "$i" -gt 0 ] && [ -z "$NO_RESET" ]; then
            echo ""
            echo "── Resetting workspace state before project: $project ──"
            cleanup_e2e_worktrees
            reset_e2e_database
        fi
        echo ""
        echo "── Running project: $project ──"
        "${CMD[@]}" --project="$project"
    done
fi
