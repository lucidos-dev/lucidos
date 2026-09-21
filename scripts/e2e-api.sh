#!/bin/bash
# Run Rust API e2e tests against the e2e-test workspace.
#
# Usage:
#   ./scripts/e2e-api.sh [options] [-- cargo test args]
#
# Options:
#   -f <filter>      Filter tests by name (passed to cargo test as filter)
#   --no-reset       Skip DB reset AND leave the workspace running for the next
#                    invocation. Use for fast iteration on a single test.
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

setup_e2e_session e2e-api

echo "Running API e2e tests (port $VITE_PORT)"

cd "$PROJECT_DIR"

export E2E_WORKSPACE
# The gateway chain test below reads all three: where the workspace is, which
# port its engine answers on, and whether that hop is TLS.
export VITE_PORT
export PROTO

# The CLI tests shell out to the `lucidos` binary. Make sure it's built and at
# the expected target path before tests run.
cargo build --locked -p lucidos-cli

CMD=(cargo test --locked -p lucidos-e2e --test api)
[ -n "$FILTER" ] && CMD+=("$FILTER")
[ ${#CARGO_ARGS[@]} -gt 0 ] && CMD+=("--" "${CARGO_ARGS[@]}")

"${CMD[@]}"

# The gateway chain test, which is the only one that puts a real gateway in
# front of a real engine. It lives in lucidos-gateway rather than lucidos-e2e
# because that crate is bin-only, so nothing outside it can build its router.
# `--ignored` keeps it out of `make test`, which has no engine to serve it. Run
# by name, so a second ignored test cannot join this step by accident.
# Skipped when a filter asked for something else.
if [ -z "$FILTER" ]; then
    echo "Running the gateway chain test (an app's own files behind a gateway)"
    cargo test --locked -p lucidos-gateway -- --ignored --exact \
        chain_tests::an_app_frames_own_files_load_through_a_gateway
fi
