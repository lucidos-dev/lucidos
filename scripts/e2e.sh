#!/bin/bash
# Run the full e2e suite — API tests, browser tests, then heavy integration
# suites that need external setup (WASM signer artifacts; real fastembed model).
#
# Usage:
#   ./scripts/e2e.sh [--packaged]
#
# --packaged (or LUCIDOS_E2E_PACKAGED=1) appends the macOS packaged-build boot
# smoke test (scripts/e2e-packaged.sh) as a final phase. OFF by default: it does a
# full release + DMG build (heavy + a Postgres download), too costly for every
# run, so only the nightly opts in. It boots the bundle's own embedded stack on
# its own port — independent of the e2e-test workspace below.
#
# Holds the e2e lock and the workspace lifecycle (engine + Vite) for the
# duration of the API + browser phases — sub-scripts detect
# $LUCIDOS_E2E_UMBRELLA and skip their own lifecycle work, so the workspace
# is booted once instead of twice. The wasm + embedder phases don't need
# the workspace, but run inside the same lock so a second concurrent
# `./scripts/e2e.sh` doesn't race the WASM build. `set -e` means an early
# failure short-circuits the rest.
#
# For granular runs (single suite, single test), use the sub-scripts directly:
#   ./scripts/e2e-api.sh [-f filter]
#   ./scripts/e2e-browser.sh [-h] [-f file] [--webkit]
#   ./scripts/e2e-wasm.sh
#   ./scripts/e2e-embedder.sh
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Opt-in: append the packaged-build boot smoke test as a final phase (default off).
RUN_PACKAGED="${LUCIDOS_E2E_PACKAGED:-0}"
while [ $# -gt 0 ]; do
    case "$1" in
        --packaged) RUN_PACKAGED=1; shift ;;
        *) echo "e2e.sh: unknown argument: $1" >&2; exit 1 ;;
    esac
done

source "$SCRIPT_DIR/lib/e2e.sh"

acquire_e2e_lock e2e || exit 1
kill_orphan_simulator
ensure_workspace_running
teardown_e2e() {
    # Belt-and-suspenders: e2e-browser.sh stops its own reaper on exit, but if it
    # died on an untrapped signal the loop is orphaned — reap it here via the
    # pidfile. No-op when nothing is running.
    stop_webkit_reaper
    cleanup_e2e_worktrees
    stop_e2e_workspace
    release_e2e_lock
}
trap teardown_e2e EXIT
trap 'exit 130' INT TERM

cleanup_e2e_worktrees
reset_e2e_database

# Read by setup_e2e_session in sub-scripts to skip their own lifecycle work.
export LUCIDOS_E2E_UMBRELLA=1

echo "═══════════════════════════════════════════════════"
echo "  Running API e2e tests"
echo "═══════════════════════════════════════════════════"
"$SCRIPT_DIR/e2e-api.sh"

echo ""
echo "═══════════════════════════════════════════════════"
echo "  Running browser e2e tests"
echo "═══════════════════════════════════════════════════"
# The API phase populated the workspace DB with throwaway threads. The browser
# phase's FIRST project (mobile-webkit) deliberately skips its own DB reset on
# the assumption the workspace was just freshly booted (see e2e-browser.sh —
# only projects 2+ reset). Under this umbrella that assumption is false: the API
# phase ran first, so mobile-webkit would inherit hundreds of API-phase threads
# and fail drawer-order-sensitive specs (e.g. threads.spec.ts "thread loads with
# correct messages when clicked" picked an API thread as the first drawer row).
# Reset here so mobile-webkit gets the clean DB it expects.
reset_e2e_database
"$SCRIPT_DIR/e2e-browser.sh"

echo ""
echo "═══════════════════════════════════════════════════"
echo "  Running wasm signer e2e tests"
echo "═══════════════════════════════════════════════════"
"$SCRIPT_DIR/e2e-wasm.sh"

echo ""
echo "═══════════════════════════════════════════════════"
echo "  Running real-embedder integration tests"
echo "═══════════════════════════════════════════════════"
"$SCRIPT_DIR/e2e-embedder.sh"

# Packaged-build boot smoke test — opt-in (--packaged / LUCIDOS_E2E_PACKAGED=1).
# Heavy (full release + DMG build); it boots the bundle's own embedded stack on
# its own port and cleans up after itself, independent of the e2e-test workspace.
if [ "$RUN_PACKAGED" = "1" ]; then
    echo ""
    echo "═══════════════════════════════════════════════════"
    echo "  Running packaged build boot smoke test"
    echo "═══════════════════════════════════════════════════"
    "$SCRIPT_DIR/e2e-packaged.sh"
fi
