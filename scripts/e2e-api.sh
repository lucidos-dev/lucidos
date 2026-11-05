#!/bin/bash
# Run Rust API e2e tests against the e2e-test workspace.
#
# Usage:
#   ./scripts/e2e-api.sh [options] [-- cargo test args]   # a whole suite on GitHub's runners, ADR 0386
#   ./scripts/e2e-api.sh -f <filter> [options]            # a filtered run on this host
#   ./scripts/e2e-api.sh --local [options]                # on this host
#
# Options:
#   --local          Run on this host, not on GitHub. Local-only flags imply it.
#   --github         Run on GitHub, even a filtered run.
#   -f <filter>      Filter tests by name (passed to cargo test as filter)
#   --no-reset       Skip DB reset AND leave the workspace running for the next
#                    invocation. Use for fast iteration on a single test.
#   --               Everything after this is passed to cargo test
#
# Examples:
#   ./scripts/e2e-api.sh                               # All API tests
#   ./scripts/e2e-api.sh -f health                      # Run only health tests
#   ./scripts/e2e-api.sh -- --nocapture                 # Show test output
#
# A local run takes a free e2e slot (scripts/lib/e2e_lock.sh), so two runs
# from two worktrees run side by side. ADR 0536.
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/lib/e2e_github.sh
source "$SCRIPT_DIR/lib/e2e_github.sh"
e2e_github_handoff api "$@"
set -- ${E2E_ARGS[@]+"${E2E_ARGS[@]}"}
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
export E2E_SNAPSHOT_ENGINE_BIN="$ENGINE_BIN"
E2E_SNAPSHOT_TARGET_DIR="$(cargo metadata --locked --no-deps --format-version 1 | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')"
export E2E_SNAPSHOT_CLI_BIN="$E2E_SNAPSHOT_TARGET_DIR/debug/lucidos"
if [ "${E2E_SNAPSHOT_BROWSER:-}" = "chromium" ]; then
    # Resolve before the fixture enters its private HOME; never download a
    # second browser or expose the user's HOME to the fixture engines.
    E2E_SNAPSHOT_BROWSER_EXECUTABLE="$(node --input-type=module -e 'import { chromium } from "@playwright/test"; process.stdout.write(chromium.executablePath());')"
    export E2E_SNAPSHOT_BROWSER_EXECUTABLE
fi

CMD=(cargo test --locked -p lucidos-e2e --test api)
# The bin-only gateway chain fixture is also selectable for focused iteration.
# Do not send its name to the API binary, where it would silently run zero tests.
if [ "$FILTER" = "chain_tests::an_app_frames_own_files_load_through_a_gateway" ] || \
   [ "$FILTER" = "server::snapshot_roundtrip_tests::isolated_snapshot_cli_roundtrip" ]; then
    CMD=(cargo test --locked -p lucidos-gateway "$FILTER")
    CARGO_ARGS=(--ignored --exact "${CARGO_ARGS[@]}")
else
    [ -n "$FILTER" ] && CMD+=("$FILTER")
fi
[ ${#CARGO_ARGS[@]} -gt 0 ] && CMD+=("--" "${CARGO_ARGS[@]}")

TREE_BEFORE="$(e2e_workspace_tree_status)" || {
    echo "ERROR: could not read the e2e workspace tree at $E2E_WORKSPACE" >&2
    exit 1
}

TESTS_RC=0
"${CMD[@]}" || TESTS_RC=$?

# A test that leaves the workspace tree dirty fails every later apply, so the
# run fails here, naming what was left, rather than in some other test. It runs
# after a failed test run too: a test that panics is the likeliest to strand a
# fixture. The test failure still decides the exit code.
TREE_RC=0
assert_e2e_workspace_tree_clean "$TREE_BEFORE" || TREE_RC=$?
[ "$TESTS_RC" -eq 0 ] || exit "$TESTS_RC"
[ "$TREE_RC" -eq 0 ] || exit "$TREE_RC"

# Real gateway/engine fixtures live in lucidos-gateway rather than lucidos-e2e
# because that crate is bin-only, so nothing outside it can build its router.
# `--ignored` keeps them out of `make test`, which has no built engine. Run
# each by name, so another ignored test cannot join this step by accident.
# Skipped when a filter asked for something else.
if [ -z "$FILTER" ]; then
    # API tests exercise workspace-wide preferences and parked agent turns.
    # The chain's live child needs the same clean queue and defaults as a
    # focused chain run, rather than inheriting the preceding suite's state.
    reset_e2e_database
    echo "Running the gateway chain test (an app's own files behind a gateway)"
    cargo test --locked -p lucidos-gateway -- --ignored --exact \
        chain_tests::an_app_frames_own_files_load_through_a_gateway
    echo "Running isolated snapshot CLI restore and Undo (private workspaces and database)"
    cargo test --locked -p lucidos-gateway -- --ignored --exact \
        server::snapshot_roundtrip_tests::isolated_snapshot_cli_roundtrip
fi
