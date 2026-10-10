#!/usr/bin/env bash
#
# check-eval-not-a-test.sh - I4: the context-mode eval never runs under
# `cargo test`.
#
#   ./scripts/check-eval-not-a-test.sh      # or: make lint-eval (part of `make lint`)
#
# WHY. ADR 0087 decision 15: a sequence run is roughly $45 and a confirmatory
# run near $1,000. The repo's test-selection rule sends every Rust change to the
# engine suite, so an eval reachable from `cargo test` would spend four figures
# on a lint fix. The eval is a binary, driven by scripts/eval-context-mode.sh.
#
# WHAT IS CHECKED, and why it is not the plan's literal rule. The plan writes I4
# as "no #[test] and no tests/ directory in the crate", and then verifies five
# other invariants with unit tests in that same crate. Both cannot hold. What
# I4 is actually protecting is that nothing `cargo test` reaches can boot a
# workspace, drive a thread, drop a database or call a model provider. So:
#
#   1. No tests/ directory, and no [lib] / [[test]] / [[bench]] in Cargo.toml.
#      `cargo test -p lucidos-eval` then has exactly one target, the binary's
#      own unit tests.
#   2. No #[cfg(test)] in main.rs, which is where the run loop lives.
#   3. At most one #[cfg(test)] per file, so "from the marker to end of file"
#      is exactly the test region and rule 4 can read it.
#   4. No test region names a spending entrypoint. A test that cannot name one
#      cannot call one.
#
# That is stronger than the literal rule in one direction (it also catches a
# tests/ file) and weaker in another (free arithmetic may be tested). The
# weakening is the point: I1, I3, I5, I8 and I10 are unit tests.
#
# Exit status: 0 when clean, non-zero otherwise, including when the crate is
# missing. A gate that cannot run must never read as clean.

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
cd "$PROJECT_DIR" || exit 1

CRATE="crates/lucidos-eval"

# Every function that spends money, boots an engine, drops a database or clears
# a data tree. Reached only from main.rs's command dispatch, never from a test.
SPENDING_ENTRYPOINTS=(
    run_repeat
    seed_repeat
    drive_task
    boot_engine
    migrate_by_booting
    recreate_database
    install_fixture_tree
    apply_seed_sql
    judge_call
    judge_vote
    judge_score
)

FAILURES=0

fail() {
    echo "FAIL: $1" >&2
    FAILURES=$((FAILURES + 1))
}

if [ ! -d "$CRATE" ]; then
    echo "ERROR: $CRATE is missing, so I4 cannot be checked." >&2
    exit 1
fi

if [ -d "$CRATE/tests" ]; then
    fail "$CRATE/tests/ exists. An integration-test target runs under \`cargo test\`."
fi

for forbidden in '\[lib\]' '\[\[test\]\]' '\[\[bench\]\]'; do
    if grep -qE "^$forbidden" "$CRATE/Cargo.toml"; then
        fail "$CRATE/Cargo.toml declares $forbidden. The crate is a bin and nothing else."
    fi
done

if grep -q '#\[cfg(test)\]' "$CRATE/src/main.rs"; then
    fail "$CRATE/src/main.rs has a #[cfg(test)] module. The run loop lives there."
fi

while IFS= read -r file; do
    markers=$(grep -c '#\[cfg(test)\]' "$file")
    if [ "$markers" -gt 1 ]; then
        fail "$file has $markers #[cfg(test)] markers. One per file, last in the file, \
so the test region is unambiguous."
        continue
    fi
    [ "$markers" -eq 0 ] && continue
    start=$(grep -n '#\[cfg(test)\]' "$file" | head -1 | cut -d: -f1)
    region=$(tail -n "+$start" "$file")
    for symbol in "${SPENDING_ENTRYPOINTS[@]}"; do
        if printf '%s\n' "$region" | grep -q "\b$symbol\b"; then
            fail "$file's test region names \`$symbol\`, which spends money or touches a \
workspace. A test must not be able to reach it."
        fi
    done
done < <(find "$CRATE/src" -name '*.rs' | sort)

if [ "$FAILURES" -gt 0 ]; then
    echo "" >&2
    echo "I4 (docs/adr/0087-context-mode-eval-graduation-bar.md, decision 15): the eval is \
a binary, never a test." >&2
    exit 1
fi

echo "OK: $CRATE cannot run the eval under \`cargo test\`."
