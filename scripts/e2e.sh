#!/bin/bash
# Run the full e2e suite — API tests then browser tests.
#
# Usage:
#   ./scripts/e2e.sh
#
# Holds the e2e lock and the workspace lifecycle (engine + Vite) for the
# duration of both phases — sub-scripts detect $LUCIDOS_E2E_UMBRELLA and
# skip their own lifecycle work, so the workspace is booted once instead
# of twice. set -e means an API failure short-circuits browser tests.
#
# For granular runs (single suite, single test), use the sub-scripts directly:
#   ./scripts/e2e-api.sh [-f filter]
#   ./scripts/e2e-browser.sh [-h] [-f file] [--webkit]
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

source "$SCRIPT_DIR/lib/e2e.sh"

acquire_e2e_lock e2e || exit 1
kill_orphan_simulator
ensure_workspace_running
teardown_e2e() {
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
"$SCRIPT_DIR/e2e-browser.sh"
