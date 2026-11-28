#!/bin/bash
# Tests for scripts/lib/test_skip.sh and how scripts/test-scripts.sh reports a
# skipped suite. Hermetic: everything runs in a mktemp sandbox.
#
# Run: ./scripts/lib/test_skip_test.sh
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SANDBOX="$(mktemp -d)"
trap 'rm -rf "$SANDBOX"' EXIT

PASS=0
FAIL=0
pass() { echo "  ok:   $*"; PASS=$((PASS + 1)); }
fail() { echo "  FAIL: $*"; FAIL=$((FAIL + 1)); }

# shellcheck source=scripts/lib/test_skip.sh
source "$SCRIPT_DIR/test_skip.sh"

echo "test: a missing subject skips the suite, a present one lets it run"
mkdir -p "$SANDBOX/root/scripts"
touch "$SANDBOX/root/scripts/here.sh"
out="$(test_skip_unless_present "$SANDBOX/root" scripts/here.sh scripts/gone.sh; echo ran)"
rc=$?
if [ "$rc" = "$TEST_SKIP_EXIT" ] && [ "$out" = "SKIP: scripts/gone.sh is not in this tree (the public mirror strips it)" ]; then
    pass "the first missing path is named, and nothing after it ran"
else
    fail "rc=$rc out=$out"
fi
out="$(test_skip_unless_present "$SANDBOX/root" scripts/here.sh; echo ran)"
if [ "$out" = ran ]; then pass "every path present: the suite runs"; else fail "present paths: $out"; fi

echo "test: a macOS-only subject skips everywhere else"
out="$(uname() { echo Linux; }; test_skip_unless_darwin build-dmg.sh; echo ran)"
rc=$?
if [ "$rc" = "$TEST_SKIP_EXIT" ] && [ "$out" = "SKIP: build-dmg.sh runs only on macOS" ]; then pass "Linux skips"; else fail "Linux: rc=$rc out=$out"; fi
out="$(uname() { echo Darwin; }; test_skip_unless_darwin build-dmg.sh; echo ran)"
if [ "$out" = ran ]; then pass "macOS runs it"; else fail "macOS: $out"; fi

echo "test: the runner reports a skip as SKIP, never as a pass"
repo="$SANDBOX/repo"
mkdir -p "$repo/scripts/lib"
cp "$SCRIPT_DIR/../test-scripts.sh" "$repo/scripts/"
cp "$SCRIPT_DIR/test_skip.sh" "$repo/scripts/lib/"
printf 'exit 0\n' >"$repo/scripts/lib/a_test.sh"
printf 'echo "SKIP: why"\nexit %s\n' "$TEST_SKIP_EXIT" >"$repo/scripts/lib/b_test.sh"
git -C "$repo" init -q
git -C "$repo" add -A
out="$(bash "$repo/scripts/test-scripts.sh")"
rc=$?
if [ "$rc" = 0 ] && printf '%s\n' "$out" | grep -q -x 'SKIP  scripts/lib/b_test.sh: SKIP: why' \
    && printf '%s\n' "$out" | grep -q '2 total, 1 passed, 0 failed, 1 skipped'; then
    pass "one pass, one skip, exit 0"
else
    fail "rc=$rc out=$out"
fi

echo "test: suites that need a release-only file skip on the stripped mirror"
mirror="$SANDBOX/mirror"
mkdir -p "$mirror"
cp -R "$SCRIPT_DIR/.." "$mirror/scripts"
# The mirror strips release_tree.sh too, so there the copy is already stripped.
if [ -f "$SCRIPT_DIR/release_tree.sh" ]; then
    while IFS= read -r path; do
        rm -rf "${mirror:?}/$path"
    done < <(
        # shellcheck source=scripts/lib/release_tree.sh
        source "$SCRIPT_DIR/release_tree.sh"
        printf '%s\n' "${RELEASE_TREE_EXCLUDE_PATHS[@]}"
    )
fi
for suite in local_channel_test.sh release_version_test.sh; do
    bash "$mirror/scripts/lib/$suite" >/dev/null 2>&1
    rc=$?
    if [ "$rc" = 0 ] || [ "$rc" = "$TEST_SKIP_EXIT" ]; then pass "$suite: rc=$rc"; else fail "$suite on the stripped tree: rc=$rc"; fi
done

echo
echo "test_skip_test: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
