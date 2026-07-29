#!/usr/bin/env bash
# Tests for the lightweight CLI/contract of scripts/build-headless.sh — the
# Tauri-free headless tarball build path (step 2 of
# docs/plans/2026-06-30-installer-step2-linux-tarball.md). Every case below
# exits BEFORE any frontend/cargo/PG build, so the whole suite is offline + fast.
# Run: ./scripts/lib/build_headless_test.sh
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
BUILD_HEADLESS="$PROJECT_DIR/scripts/build-headless.sh"

PASS=0
FAIL=0
fail() { echo "  FAIL: $*"; FAIL=$((FAIL+1)); }
pass() { echo "  ok:   $*"; PASS=$((PASS+1)); }

echo "test: --check validates the resource contract and exits 0"
out="$("$BUILD_HEADLESS" --check 2>&1)"; rc=$?
if [ $rc -eq 0 ]; then pass "--check exits 0"; else fail "--check exited $rc: $out"; fi
for name in lucidos-engine lucidos-gateway frontend postgres sdk system-knowhow; do
    if echo "$out" | grep -q "$name"; then pass "mentions $name"; else fail "missing $name from --check: $out"; fi
done
# The `lucidos` CLI is a distinct bundled resource — assert the space-delimited
# standalone token so this doesn't pass merely by matching the lucidos-engine /
# lucidos-gateway prefix (`grep -w` would: `-` is a non-word char, so `-w lucidos`
# matches inside `lucidos-engine`).
if echo "$out" | grep -qE '(^| )lucidos( |$)'; then pass "mentions the lucidos CLI"; else fail "missing the lucidos CLI from --check: $out"; fi

echo ""
echo "test: --check resolves a host triple"
out="$("$BUILD_HEADLESS" --check 2>&1)"
if echo "$out" | grep -Eq 'headless bundle for [a-z0-9_]+-(apple-darwin|unknown-linux-gnu)'; then
    pass "--check names a resolved triple"
else
    fail "no resolved triple in: $out"
fi

echo ""
echo "test: --check --triple <linux> validates the Linux contract offline"
# The must-work Linux triple (parent plan). --check never builds, so this works on
# any host — proving the Linux contract is reachable without a Linux box.
out="$("$BUILD_HEADLESS" --check --triple x86_64-unknown-linux-gnu 2>&1)"; rc=$?
if [ $rc -eq 0 ] && echo "$out" | grep -q "x86_64-unknown-linux-gnu"; then
    pass "--check --triple x86_64-unknown-linux-gnu exits 0 for the Linux contract"
else
    fail "expected the Linux contract to validate offline (rc=$rc): $out"
fi

echo ""
echo "test: a cross-triple BUILD is refused fast (compiles natively)"
# A real build for a non-host triple can't work (native compile), so it must fail
# fast with a clear message BEFORE any cargo/npm/network work.
host_arch="$(uname -m)"
case "$host_arch" in
    arm64|aarch64) other="x86_64-unknown-linux-gnu" ;;   # never the host on arm Macs/Linux
    *)             other="aarch64-unknown-linux-gnu" ;;
esac
out="$("$BUILD_HEADLESS" --triple "$other" 2>&1)"; rc=$?
if [ $rc -ne 0 ] && echo "$out" | grep -qi "compiles natively"; then
    pass "cross-triple build refused with a clear message"
else
    fail "expected a fast cross-triple refusal (rc=$rc): $out"
fi

echo ""
echo "test: unknown argument is rejected"
out="$("$BUILD_HEADLESS" --bogus-flag 2>&1)"; rc=$?
if [ $rc -ne 0 ] && echo "$out" | grep -q "unknown argument"; then
    pass "unknown argument exits non-zero with a clear message"
else
    fail "expected unknown-argument rejection; got rc=$rc: $out"
fi

echo ""
echo "test: flags requiring an argument reject a missing value"
for flag in --triple --out-dir --version; do
    out="$("$BUILD_HEADLESS" "$flag" 2>&1)"; rc=$?
    if [ $rc -ne 0 ] && echo "$out" | grep -q "requires an argument"; then
        pass "$flag without a value is refused"
    else
        fail "expected '$flag requires an argument' (rc=$rc): $out"
    fi
done

echo ""
echo "test: --help documents the flags"
out="$("$BUILD_HEADLESS" --help 2>&1)"
for token in -- --triple --out-dir --version --check; do
    if echo "$out" | grep -q -- "$token"; then pass "--help documents $token"; else fail "--help missing $token"; fi
done

echo ""
echo "test: build-headless.sh keeps the errtrace + ERR-safe contract (set -Eeuo)"
# Like build-dmg.sh, a failing stage must not exit silently. build-headless uses a
# plain die() (no cockpit events), but set -Eeuo pipefail is still the floor.
if grep -q 'set -Eeuo pipefail' "$BUILD_HEADLESS"; then
    pass "build-headless.sh sets errtrace (set -Eeuo pipefail)"
else
    fail "build-headless.sh missing 'set -Eeuo pipefail'"
fi

echo ""
echo "test: the shared staging lib + its test ship alongside build-headless.sh"
for f in scripts/lib/stage_runtime.sh scripts/lib/stage_runtime_test.sh scripts/build-headless.sh; do
    if [ -f "$PROJECT_DIR/$f" ]; then pass "$f exists"; else fail "$f is missing"; fi
done

echo ""
echo "build_headless: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
