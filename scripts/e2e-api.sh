#!/bin/bash
# Run Rust API e2e tests against the e2e-test workspace.
#
# Usage:
#   ./scripts/e2e-api.sh [options] [-- cargo test args]
#
# Options:
#   -f <filter>      Filter tests by name (passed to cargo test as filter)
#   --no-reset       Skip database reset
#   --               Everything after this is passed to cargo test
#
# Examples:
#   ./scripts/e2e-api.sh                               # All API tests
#   ./scripts/e2e-api.sh -f health                      # Run only health tests
#   ./scripts/e2e-api.sh -- --nocapture                 # Show test output
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"

source "$SCRIPT_DIR/lib/e2e.sh"

FILTER=""
NO_RESET=""
CARGO_ARGS=()

while [[ $# -gt 0 ]]; do
    case "$1" in
        -f) FILTER="$2"; shift 2 ;;
        --no-reset) NO_RESET=1; shift ;;
        --) shift; CARGO_ARGS+=("$@"); break ;;
        *) CARGO_ARGS+=("$1"); shift ;;
    esac
done

acquire_e2e_lock e2e-api || exit 1
ensure_workspace_running
teardown_e2e() {
    stop_e2e_workspace
    release_e2e_lock
}
trap teardown_e2e EXIT
trap 'exit 130' INT TERM
[ -z "$NO_RESET" ] && cleanup_e2e_worktrees
[ -z "$NO_RESET" ] && reset_e2e_database

echo "Running API e2e tests (port $VITE_PORT)"

cd "$PROJECT_DIR"

export E2E_WORKSPACE

# The CLI tests in api_e2e shell out to the `cognos` binary. Make sure it's
# built and at the expected target path before tests run.
cargo build -p cognos-cli

CMD=(cargo test -p cognos-engine --test api_e2e)
[ -n "$FILTER" ] && CMD+=("$FILTER")
CMD+=("--" "--ignored")
[ ${#CARGO_ARGS[@]} -gt 0 ] && CMD+=("${CARGO_ARGS[@]}")

"${CMD[@]}"
