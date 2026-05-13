#!/bin/bash
# Build signer WASM artifacts then run the wasm signer e2e tests.
#
# Usage:
#   ./scripts/e2e-wasm.sh [-- cargo test args]
#
# Builds every signer in `signers/` to `wasm32-unknown-unknown` release mode
# (via `./signers/build-all.sh`) so the tests in
# `crates/lucidos-e2e/tests/wasm_signers.rs` can find the resulting `.wasm`
# artifacts. Skipping the build means the test panics with a clear
# "did you run `./signers/build-all.sh`?" message.
#
# These tests do NOT need a running Lucidos workspace — they are pure Rust
# integration tests that load the WASM modules directly.
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"

CARGO_ARGS=()
while [[ $# -gt 0 ]]; do
    case "$1" in
        --) shift; CARGO_ARGS+=("$@"); break ;;
        *)  CARGO_ARGS+=("$1"); shift ;;
    esac
done

cd "$PROJECT_DIR"

echo "Building signer WASM artifacts..."
./signers/build-all.sh

echo "Running wasm signer e2e tests..."
CMD=(cargo test -p lucidos-e2e --test wasm_signers)
[ ${#CARGO_ARGS[@]} -gt 0 ] && CMD+=("--" "${CARGO_ARGS[@]}")
"${CMD[@]}"
