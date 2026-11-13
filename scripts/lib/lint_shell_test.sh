#!/bin/bash
# Tests for scripts/lint-shell.sh: it runs ShellCheck in bounded batches, and
# one failing batch fails the gate without hiding any other batch's findings.
#
# Hermetic: the script runs from a copy inside a throwaway git repo, and a stub
# `shellcheck` on PATH records the argv of every call. The real ShellCheck
# never runs, so the outcome does not drift with the real tree.
#
# Run: ./scripts/lib/lint_shell_test.sh
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
LINT="$SCRIPT_DIR/../lint-shell.sh"

PASS=0
FAIL=0
pass() { echo "  ok:   $*"; PASS=$((PASS + 1)); }
fail() { echo "  FAIL: $*"; FAIL=$((FAIL + 1)); }

# Pin read from the script, so the test never restates the batch size.
BATCH_SIZE="$(sed -n 's/^SHELLCHECK_BATCH_SIZE=\([0-9][0-9]*\)$/\1/p' "$LINT")"
if [ -z "$BATCH_SIZE" ]; then
    echo "FAIL: lint-shell.sh defines no SHELLCHECK_BATCH_SIZE=<n> line"
    exit 1
fi

SANDBOX="$(mktemp -d)"
trap 'rm -rf "$SANDBOX"' EXIT

# The stub writes one line per call: the file arguments, space separated. A
# file whose name starts with "bad" yields a finding and exit status 1.
mkdir -p "$SANDBOX/bin"
cat > "$SANDBOX/bin/shellcheck" <<'EOF'
#!/bin/bash
if [ "${1:-}" = "--version" ]; then
    echo "version: 0.0.0-stub"
    exit 0
fi
echo "$*" >> "$SHELLCHECK_CALLS"
rc=0
for f in "$@"; do
    case "$(basename "$f")" in
        bad*) echo "In $f line 1: stub finding"; rc=1 ;;
    esac
done
exit "$rc"
EOF
chmod +x "$SANDBOX/bin/shellcheck"

# A fresh repo holding a copy of lint-shell.sh, .shellcheckrc, and the given
# number of clean scripts plus any named extras.
make_repo() { # <clean-count> [extra-file...]
    REPO="$(mktemp -d "$SANDBOX/repo.XXXXXX")"
    git -C "$REPO" init -q -b main
    mkdir -p "$REPO/scripts"
    cp "$LINT" "$REPO/scripts/lint-shell.sh"
    touch "$REPO/.shellcheckrc"
    local i=1
    while [ "$i" -le "$1" ]; do
        printf '#!/bin/bash\n' > "$REPO/$(printf 'clean%03d.sh' "$i")"
        i=$((i + 1))
    done
    shift
    local extra
    for extra in "$@"; do
        mkdir -p "$REPO/$(dirname "$extra")"
        printf '#!/bin/bash\n' > "$REPO/$extra"
    done
    git -C "$REPO" add -A
}

run_lint() {
    SHELLCHECK_CALLS="$SANDBOX/calls.$$.$RANDOM"
    : > "$SHELLCHECK_CALLS"
    OUT=$(cd "$REPO" && SHELLCHECK_CALLS="$SHELLCHECK_CALLS" PATH="$SANDBOX/bin:$PATH" \
        bash scripts/lint-shell.sh 2>&1)
    RC=$?
}

# Every call names at most BATCH_SIZE files, and every tracked file is named
# exactly once, in discovery order.
test_batches_are_bounded_and_cover_every_file_once() {
    echo "test: shellcheck gets at most $BATCH_SIZE files per call, every file once"
    make_repo $((BATCH_SIZE * 2 + 3))
    run_lint
    local max=0 n line
    while IFS= read -r line; do
        # shellcheck disable=SC2086 # word splitting counts the file arguments
        set -- $line
        n=$#
        [ "$n" -gt "$max" ] && max=$n
    done < "$SHELLCHECK_CALLS"
    local calls
    calls=$(wc -l < "$SHELLCHECK_CALLS" | tr -d ' ')
    if [ "$max" -le "$BATCH_SIZE" ] && [ "$calls" -eq 3 ]; then
        pass "$calls calls, largest has $max files"
    else
        fail "expected 3 calls of at most $BATCH_SIZE files, got $calls calls, largest $max"
    fi
    local expected got
    expected=$(git -C "$REPO" ls-files '*.sh' | tr '\n' ' ')
    got=$(tr '\n' ' ' < "$SHELLCHECK_CALLS")
    if [ "$got" = "$expected" ]; then
        pass "every file checked once, in discovery order"
    else
        fail "file coverage differs. expected: $expected got: $got"
    fi
    if [ "$RC" -eq 0 ]; then
        pass "clean tree exits 0"
    else
        fail "clean tree exited $RC: $OUT"
    fi
}

# Only the last batch fails: the gate must still fail.
test_failure_in_last_batch_fails_the_gate() {
    echo "test: a finding only in the last batch fails the gate"
    make_repo $((BATCH_SIZE * 2)) scripts/bad_last.sh
    run_lint
    if [ "$RC" -ne 0 ]; then
        pass "exit status $RC"
    else
        fail "exited 0 with a finding in the last batch"
    fi
    case "$OUT" in
        *"In scripts/bad_last.sh line 1: stub finding"*) pass "the last batch's finding is printed" ;;
        *) fail "the last batch's finding is missing: $OUT" ;;
    esac
}

# The first batch fails: later batches still run and their findings print.
test_failure_in_first_batch_does_not_stop_later_batches() {
    echo "test: a failing first batch does not stop later batches"
    make_repo $((BATCH_SIZE * 2)) bad_first.sh zz/bad_late.sh
    run_lint
    local calls
    calls=$(wc -l < "$SHELLCHECK_CALLS" | tr -d ' ')
    if [ "$RC" -ne 0 ] && [ "$calls" -eq 3 ]; then
        pass "exit status $RC after all $calls batches"
    else
        fail "expected non-zero exit after 3 batches, got exit $RC after $calls"
    fi
    case "$OUT" in
        *"In bad_first.sh line 1"*"In zz/bad_late.sh line 1"*) pass "both findings print, in order" ;;
        *) fail "expected both findings in order: $OUT" ;;
    esac
}

test_batches_are_bounded_and_cover_every_file_once
test_failure_in_last_batch_fails_the_gate
test_failure_in_first_batch_does_not_stop_later_batches

echo
echo "lint_shell_test: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
