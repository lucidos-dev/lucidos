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
echo "test: --release-build is recognized and shares the version-stamp guard"
# --release-build is a BUILD mode, so it runs the same up-front version guard as
# --release and exits fast (before any build) on a mismatch — proving it parses.
out="$("$PROJECT_DIR/scripts/build-dmg.sh" --release-build --release-version 99.99.99-build-dmg-test 2>&1)"
rc=$?
if [ $rc -ne 0 ] && echo "$out" | grep -q "version-stamp mismatch"; then
    pass "--release-build rejects a mismatched --release-version (recognized as a build mode)"
else
    fail "expected version-stamp mismatch for --release-build; got rc=$rc out: $out"
fi

echo ""
echo "test: --release-attach requires --staging-dir"
out="$("$PROJECT_DIR/scripts/build-dmg.sh" --release-attach --upload-tag v9.9.9 2>&1)"
rc=$?
if [ $rc -ne 0 ] && echo "$out" | grep -q "requires --staging-dir"; then
    pass "--release-attach without --staging-dir exits non-zero with a clear message"
else
    fail "expected --staging-dir requirement; got rc=$rc out: $out"
fi

# ── --release-attach staging guard (offline) ─────────────────────────────────
# Build a staging fixture (fake artifacts + a real manifest) and corrupt it. Each
# case below fails at staging VERIFICATION — before any gh/network/event — so the
# whole suite stays offline + signing-free.
# shellcheck source=scripts/lib/release_staging.sh
source "$PROJECT_DIR/scripts/lib/release_staging.sh"
make_staging() {
    local dir; dir="$(mktemp -d)"
    printf 'dmg\n' > "$dir/Lucidos_0.0.0_aarch64.dmg"
    printf 'tar\n' > "$dir/Lucidos.app.tar.gz"
    printf 'sig\n' > "$dir/Lucidos.app.tar.gz.sig"
    release_staging_write_manifest "$dir" 0.0.0 abc123 \
        Lucidos_0.0.0_aarch64.dmg Lucidos.app.tar.gz Lucidos.app.tar.gz.sig >/dev/null
    printf '%s' "$dir"
}

echo ""
echo "test: --release-attach refuses a staging dir with no manifest"
EMPTY="$(mktemp -d)"
out="$("$PROJECT_DIR/scripts/build-dmg.sh" --release-attach --staging-dir "$EMPTY" --upload-tag v9.9.9 2>&1)"
rc=$?
if [ $rc -ne 0 ] && echo "$out" | grep -qi "manifest"; then
    pass "missing manifest is refused"
else
    fail "expected missing-manifest refusal; got rc=$rc out: $out"
fi
rm -rf "$EMPTY"

echo ""
echo "test: --release-attach refuses a missing staged artifact"
S="$(make_staging)"
rm -f "$S/Lucidos.app.tar.gz.sig"
out="$("$PROJECT_DIR/scripts/build-dmg.sh" --release-attach --staging-dir "$S" --upload-tag v9.9.9 2>&1)"
rc=$?
if [ $rc -ne 0 ] && echo "$out" | grep -qi "missing"; then
    pass "missing artifact is refused"
else
    fail "expected missing-artifact refusal; got rc=$rc out: $out"
fi
rm -rf "$S"

echo ""
echo "test: --release-attach refuses a checksum-mismatched staged artifact"
S="$(make_staging)"
printf 'tampered\n' >> "$S/Lucidos_0.0.0_aarch64.dmg"
out="$("$PROJECT_DIR/scripts/build-dmg.sh" --release-attach --staging-dir "$S" --upload-tag v9.9.9 2>&1)"
rc=$?
if [ $rc -ne 0 ] && echo "$out" | grep -qi "checksum mismatch"; then
    pass "checksum mismatch is refused"
else
    fail "expected checksum-mismatch refusal; got rc=$rc out: $out"
fi
rm -rf "$S"

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
