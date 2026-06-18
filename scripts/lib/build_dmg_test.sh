#!/bin/bash
# Tests for the lightweight packaging contract in scripts/build-dmg.sh.
# Run: ./scripts/lib/build_dmg_test.sh
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"

PASS=0
FAIL=0
fail() { echo "  FAIL: $*"; FAIL=$((FAIL+1)); }
pass() { echo "  ok:   $*"; PASS=$((PASS+1)); }

echo "test: build-dmg resource contract includes packaged gateway stack"
out="$("$PROJECT_DIR/scripts/build-dmg.sh" --check 2>&1)"
rc=$?

if [ $rc -eq 0 ]; then
    pass "--check exits 0"
else
    fail "--check exited $rc; output: $out"
fi

for name in lucidos-gateway lucidos-engine frontend postgres sdk; do
    if echo "$out" | grep -q "$name"; then
        pass "mentions $name"
    else
        fail "missing $name from --check output: $out"
    fi
done

echo ""
echo "test: --release version-stamp guard rejects a release-version != RELEASE"
# The guard runs right after arg parsing — before the Darwin/tooling checks and
# the build — so this exits fast and never starts a build. CURRENT_STEP is unset
# at that point, so no event is emitted (no engine round-trip).
out="$("$PROJECT_DIR/scripts/build-dmg.sh" --release --release-version 99.99.99-build-dmg-test 2>&1)"
rc=$?
if [ $rc -ne 0 ]; then
    pass "--release with mismatched --release-version exits non-zero"
else
    fail "expected non-zero exit for version mismatch; got rc=$rc"
fi
case "$out" in
    *"version-stamp mismatch"*) pass "reports a version-stamp mismatch" ;;
    *) fail "missing version-stamp mismatch message; got: $out" ;;
esac

echo ""
echo "test: unknown argument is rejected"
out="$("$PROJECT_DIR/scripts/build-dmg.sh" --bogus-flag 2>&1)"
rc=$?
if [ $rc -ne 0 ] && echo "$out" | grep -q "unknown argument"; then
    pass "unknown argument exits non-zero with a clear message"
else
    fail "expected unknown-argument rejection; got rc=$rc out: $out"
fi

echo ""
echo "test: release scripts keep the failure-emit contract (errtrace + ERR trap)"
# A failing stage must emit ReleaseStepFailed, not exit silently. That relies on
# `set -E` (so the ERR trap inherits into shell functions) AND an `on_err` ERR
# trap. Without `-E` the trap never fires for failures inside sign/refresh/upload
# functions and the cockpit stalls — guard against a future edit dropping either.
for s in build-dmg.sh release.sh release-to-lucidos.sh; do
    f="$PROJECT_DIR/scripts/$s"
    if grep -q 'set -Eeuo pipefail' "$f"; then
        pass "$s sets errtrace (set -Eeuo pipefail)"
    else
        fail "$s missing 'set -Eeuo pipefail' (ERR trap won't fire inside functions)"
    fi
    if grep -q 'trap on_err ERR' "$f"; then
        pass "$s arms the on_err ERR trap"
    else
        fail "$s missing 'trap on_err ERR'"
    fi
done

echo ""
echo "build_dmg: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
