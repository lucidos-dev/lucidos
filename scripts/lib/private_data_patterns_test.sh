#!/bin/bash
# Tests for scripts/lib/private_data_patterns.sh: the scan finds a planted
# token, and a scan that cannot run says so rather than reading clean.
# Hermetic: a fixture repo and denylist in a mktemp sandbox.
#
# Run: ./scripts/lib/private_data_patterns_test.sh
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SANDBOX="$(mktemp -d)"
trap 'rm -rf "$SANDBOX"' EXIT
export GIT_CONFIG_NOSYSTEM=1 HOME="$SANDBOX/home"
mkdir -p "$HOME"

PASS=0
FAIL=0
pass() { echo "  ok:   $*"; PASS=$((PASS + 1)); }
fail() { echo "  FAIL: $*"; FAIL=$((FAIL + 1)); }

# shellcheck source=scripts/lib/private_data_patterns.sh
source "$SCRIPT_DIR/private_data_patterns.sh"

REPO="$SANDBOX/repo"
git init -q "$REPO"
git -C "$REPO" config user.email dev@example.com
git -C "$REPO" config user.name "A Developer"
mkdir -p "$REPO/src"
echo "see widget-pipeline" >"$REPO/src/leak.txt"
git -C "$REPO" add -A
git -C "$REPO" commit -qm plant

cat >"$SANDBOX/denylist.md" <<'FIXTURE'
<!-- BEGIN private-data-denylist -->
```
# an invented internal project name
widget-pipeline
```
<!-- END private-data-denylist -->

<!-- BEGIN private-data-exceptions -->
```
# no carve-outs
```
<!-- END private-data-exceptions -->
FIXTURE
export PRIVATE_DATA_DENYLIST_SOURCE="$SANDBOX/denylist.md"

echo "test: a planted token is a hit"
out="$(private_data_grep_tree HEAD "$REPO")"
rc=$?
if [ "$rc" = 0 ] && printf '%s\n' "$out" | grep -q 'src/leak.txt'; then
    pass "the scan ran and named the file"
else
    fail "rc=$rc out=$out"
fi

# GNU mktemp refused the BSD-only `mktemp -t name` form. The redirect to the
# empty name then failed, git grep never ran, and exit 1 read as clean.
echo "test: a scan that cannot make its temp file fails, never reads clean"
out="$(mktemp() { return 1; }; private_data_grep_tree HEAD "$REPO" 2>/dev/null)"
rc=$?
if [ "$rc" -ne 0 ] && [ -z "$out" ]; then
    pass "no temp file: the scan fails closed"
else
    fail "rc=$rc out=$out"
fi

echo
echo "private_data_patterns_test: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
