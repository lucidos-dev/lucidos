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
# The eval target requires the opt-in eval feature. Its unit tests may check
# free arithmetic and fixture handling. None may boot a workspace, drive a
# thread, drop a database or call a model provider. Other targets in the root
# package have their own tests.
#
# Each eval source file may have one #[cfg(test)] region, at the end. The check
# rejects all spending entry points in that region and keeps tests out of the
# main run loop.
#
# Exit status: 0 when clean, non-zero otherwise, including when the target is
# missing. A gate that cannot run must never read as clean.

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
cd "$PROJECT_DIR" || exit 1

PACKAGE="crates/lucidos-engine-bin"
EVAL="src/bin/lucidos-eval"

# Every function that spends money, boots an engine, drops a database or clears
# a data tree. Reached only from main.rs's command dispatch, never from a test.
SPENDING_ENTRYPOINTS=(
    run_repeat
    seed_repeat
    drive_task
    boot_engine
    migrate_by_booting
    create_database
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

if [ ! -f "$PACKAGE/$EVAL/main.rs" ]; then
    echo "ERROR: $PACKAGE/$EVAL/main.rs is missing, so I4 cannot be checked." >&2
    exit 1
fi

# Inspect the eval target only. The root package can declare other binaries,
# integration tests and benchmarks.
if ! awk -v eval_path="$EVAL/" '
    function finish() {
        if (name == "lucidos-eval" || index(path, eval_path) == 1) {
            if (table != "[[bin]]" || name != "lucidos-eval" ||
                path != eval_path "main.rs" || features != "[\"eval\"]") invalid = 1
            else found++
        }
    }
    /^[[:space:]]*\[/ {
        finish()
        table = $1
        name = path = features = ""
    }
    /^[[:space:]]*(name|path)[[:space:]]*=/ {
        value = $0
        sub(/^[^=]+=[[:space:]]*"/, "", value)
        sub(/".*$/, "", value)
        if ($0 ~ /^[[:space:]]*name/) name = value
        else path = value
    }
    /^[[:space:]]*required-features[[:space:]]*=/ {
        features = $0
        sub(/^[^=]+=[[:space:]]*/, "", features)
        sub(/[[:space:]]*(#.*)?$/, "", features)
    }
    END { finish(); exit (invalid || found != 1) }
' "$PACKAGE/Cargo.toml"; then
    fail "$PACKAGE/Cargo.toml must declare one feature-gated lucidos-eval binary for $EVAL."
fi

# A default feature list could enable eval, directly or through another feature.
if awk '/^[[:space:]]*\[/ { in_features = ($1 == "[features]") }
    in_features && /^[[:space:]]*default[[:space:]]*=/ { found = 1 }
    END { exit !found }' "$PACKAGE/Cargo.toml"; then
    fail "$PACKAGE/Cargo.toml declares default features. The eval feature must stay opt-in."
fi

if grep -q '#\[cfg(test)\]' "$PACKAGE/$EVAL/main.rs"; then
    fail "$PACKAGE/$EVAL/main.rs has a #[cfg(test)] module. The run loop lives there."
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
done < <(find "$PACKAGE/$EVAL" -name '*.rs' | sort)

if [ "$FAILURES" -gt 0 ]; then
    echo "" >&2
    echo "I4 (docs/adr/0087-context-mode-eval-graduation-bar.md, decision 15): the eval is \
a binary, never a test." >&2
    exit 1
fi

echo "OK: $PACKAGE/$EVAL cannot run the eval under \`cargo test\`."
