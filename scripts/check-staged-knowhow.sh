#!/usr/bin/env bash
# check-staged-knowhow.sh: refuse a build whose staged system-knowhow copy has
# drifted from the live <repo>/system-knowhow tree.
#
#   ./scripts/check-staged-knowhow.sh
#
# Runs from crates/lucidos-app/tauri.conf.json's beforeBuildCommand, which is the
# one hook inside EVERY `cargo tauri build` no matter who invoked it. That is the
# point: build-dmg.sh already restages from scratch (stage_runtime_assemble
# rm -rf's first), so the builds are correct. What is not correct is the leftover
# stage sitting in crates/lucidos-app/bundle-resources/ between builds, which a
# hand-run `cargo tauri build --config '<resource map>'` will happily package.
# stage_runtime_staged_knowhow_fresh's header carries the full reasoning.
#
# Ordering inside build-dmg.sh is safe: it stages at step 4 and runs
# `cargo tauri build` at step 5, so the guard always sees a stage it just wrote.
#
# Exit status: 0 when the stage is absent, carries no system-knowhow/, or matches
# the live tree. Non-zero with the diff on stderr otherwise.

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(dirname "$SCRIPT_DIR")"

# shellcheck source=scripts/lib/stage_runtime.sh
source "$SCRIPT_DIR/lib/stage_runtime.sh"

stage_runtime_staged_knowhow_fresh \
    "$REPO_ROOT/crates/lucidos-app/bundle-resources" \
    "$REPO_ROOT/system-knowhow"
