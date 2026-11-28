#!/usr/bin/env bash
# Hermetic composite-command fixtures: source functions only, execute fake tests.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/lib/harden_suites.sh
source "$SCRIPT_DIR/harden_suites.sh"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
unset HS_STUB_CMD

[ "$(hs_suite_command lint-static normal "$TMP" 1)" = "make lint-static LINT_STATIC_TARGETS='lint-shell lint-eval'" ]
[ "$(hs_suite_command lint-static normal "$TMP")" = "make lint-static" ]
echo "PASS: only static lint with completed knowhow checks omits them"

for suite in release harden-suites codegen; do
    cmd=$(hs_suite_command "$suite" normal "$TMP")
    mkdir -p "$TMP/$suite"
    paths=$(printf '%s\n' "$cmd" | grep -oE 'scripts/lib/[a-z0-9_]*_test\.sh')
    expected=0
    for path in $paths; do
        mkdir -p "$TMP/$suite/$(dirname "$path")"
        printf 'echo ran >> calls\nexit 17\n' > "$TMP/$suite/$path"
        expected=$((expected + 1))
    done
    [ "$expected" -gt 1 ]
    rc=0
    (cd "$TMP/$suite" && bash -c "$cmd") > "$TMP/log" 2>&1 || rc=$?
    [ "$rc" -ne 0 ]
    [ "$(wc -l < "$TMP/$suite/calls" | tr -d ' ')" = "$expected" ]
    echo "PASS: $suite runs all $expected failing fixtures"
    for path in $paths; do printf ':\n' > "$TMP/$suite/$path"; done
    (cd "$TMP/$suite" && bash -c "$cmd")
    echo "PASS: $suite succeeds when all fixtures succeed"
    # A missing first fixture must not hide the last fixture.
    rm "$TMP/$suite/$(printf '%s\n' "$paths" | head -1)"
    last=$(printf '%s\n' "$paths" | tail -1)
    printf 'touch last-ran\n' > "$TMP/$suite/$last"
    rc=0
    (cd "$TMP/$suite" && bash -c "$cmd") > "$TMP/log" 2>&1 || rc=$?
    [ "$rc" -ne 0 ]
    [ -f "$TMP/$suite/last-ran" ]
    echo "PASS: $suite retains missing-fixture failure and runs later fixtures"
done

for pair in 'harden-gates harden_gates' 'build-prerequisites build_prerequisites'; do
    read -r command library <<< "$pair"
    actual=$(printf 'scripts/%s.sh\n' "$command" | hs_script_lib_tests)
    [ "$actual" = "scripts/lib/${library}_test.sh" ]
    echo "PASS: $command selects its regression fixtures"
done
