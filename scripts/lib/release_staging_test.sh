#!/usr/bin/env bash
# Tests for scripts/lib/release_staging.sh — the staging-manifest helpers that
# back the build-once / verify-then-publish flow. Pure functions (python3 hashlib
# + json, no git/gh/network), so the whole matrix runs offline with fake artifact
# files. Run: ./scripts/lib/release_staging_test.sh
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/lib/release_staging.sh
source "$SCRIPT_DIR/release_staging.sh"

PASS=0
FAIL=0
fail() { echo "  FAIL: $*"; FAIL=$((FAIL+1)); }
pass() { echo "  ok:   $*"; PASS=$((PASS+1)); }

# A staging dir with three fake artifacts (names mirror the real release set).
new_staging() {
    local dir
    dir="$(mktemp -d)"
    printf 'fake dmg payload\n'      > "$dir/Lucidos_0.12.2_aarch64.dmg"
    printf 'fake updater tarball\n'  > "$dir/Lucidos.app.tar.gz"
    printf 'fake updater signature\n'> "$dir/Lucidos.app.tar.gz.sig"
    printf '%s' "$dir"
}

VERSION="0.12.2"
COMMIT="deadbeefcafef00dba5eba11c0ffee1234567890"

# ── write_manifest: correct version / source_commit / sha256 ──────────────────
echo "test: write_manifest records version, source_commit, and per-artifact sha256"
DIR="$(new_staging)"
if release_staging_write_manifest "$DIR" "$VERSION" "$COMMIT" \
        Lucidos_0.12.2_aarch64.dmg Lucidos.app.tar.gz Lucidos.app.tar.gz.sig; then
    pass "write_manifest succeeds"
else
    fail "write_manifest returned non-zero"
fi
[ -f "$DIR/manifest.json" ] && pass "manifest.json was created" || fail "manifest.json missing"

got_ver="$(release_staging_manifest_field "$DIR" version 2>/dev/null)"
[ "$got_ver" = "$VERSION" ] && pass "version field = $VERSION" || fail "version field wrong: '$got_ver'"

got_commit="$(release_staging_manifest_field "$DIR" source_commit 2>/dev/null)"
[ "$got_commit" = "$COMMIT" ] && pass "source_commit field = $COMMIT" || fail "source_commit wrong: '$got_commit'"

# The manifest's sha256 for the .dmg must equal an independently-computed sha256.
want_dmg_sha="$(release_staging_sha256 "$DIR/Lucidos_0.12.2_aarch64.dmg")"
if grep -q "\"$want_dmg_sha\"" "$DIR/manifest.json"; then
    pass "manifest carries the real sha256 of the .dmg"
else
    fail "manifest sha256 for the .dmg does not match the file"
fi
rm -rf "$DIR"

# ── verify: accepts an intact staging dir ─────────────────────────────────────
echo ""
echo "test: verify accepts an intact staging dir"
DIR="$(new_staging)"
release_staging_write_manifest "$DIR" "$VERSION" "$COMMIT" \
    Lucidos_0.12.2_aarch64.dmg Lucidos.app.tar.gz Lucidos.app.tar.gz.sig >/dev/null
if release_staging_verify "$DIR" 2>/dev/null; then
    pass "verify exits 0 for an intact staging dir"
else
    fail "verify rejected an intact staging dir"
fi
rm -rf "$DIR"

# ── verify: refuses a missing artifact ────────────────────────────────────────
echo ""
echo "test: verify refuses a missing artifact"
DIR="$(new_staging)"
release_staging_write_manifest "$DIR" "$VERSION" "$COMMIT" \
    Lucidos_0.12.2_aarch64.dmg Lucidos.app.tar.gz Lucidos.app.tar.gz.sig >/dev/null
rm -f "$DIR/Lucidos.app.tar.gz.sig"
out="$(release_staging_verify "$DIR" 2>&1)"; rc=$?
if [ $rc -ne 0 ] && echo "$out" | grep -qi "missing"; then
    pass "verify exits non-zero and names the missing artifact"
else
    fail "verify did not refuse a missing artifact (rc=$rc): $out"
fi
rm -rf "$DIR"

# ── verify: refuses a checksum-mismatched artifact ────────────────────────────
echo ""
echo "test: verify refuses a tampered (checksum-mismatched) artifact"
DIR="$(new_staging)"
release_staging_write_manifest "$DIR" "$VERSION" "$COMMIT" \
    Lucidos_0.12.2_aarch64.dmg Lucidos.app.tar.gz Lucidos.app.tar.gz.sig >/dev/null
printf 'tampered\n' >> "$DIR/Lucidos_0.12.2_aarch64.dmg"
out="$(release_staging_verify "$DIR" 2>&1)"; rc=$?
if [ $rc -ne 0 ] && echo "$out" | grep -qi "checksum mismatch"; then
    pass "verify exits non-zero on a checksum mismatch"
else
    fail "verify did not refuse a tampered artifact (rc=$rc): $out"
fi
rm -rf "$DIR"

# ── verify: refuses a missing manifest ────────────────────────────────────────
echo ""
echo "test: verify refuses a staging dir with no manifest"
DIR="$(new_staging)"
out="$(release_staging_verify "$DIR" 2>&1)"; rc=$?
if [ $rc -ne 0 ] && echo "$out" | grep -qi "manifest"; then
    pass "verify exits non-zero when manifest.json is absent"
else
    fail "verify accepted a dir with no manifest (rc=$rc): $out"
fi
rm -rf "$DIR"

# ── assert_commit: the identity guard ─────────────────────────────────────────
echo ""
echo "test: assert_commit accepts a matching commit, rejects a moved one"
DIR="$(new_staging)"
release_staging_write_manifest "$DIR" "$VERSION" "$COMMIT" \
    Lucidos_0.12.2_aarch64.dmg Lucidos.app.tar.gz Lucidos.app.tar.gz.sig >/dev/null
if release_staging_assert_commit "$DIR" "$COMMIT" 2>/dev/null; then
    pass "assert_commit accepts the recorded source_commit"
else
    fail "assert_commit rejected the recorded source_commit"
fi
out="$(release_staging_assert_commit "$DIR" "0000000000000000000000000000000000000000" 2>&1)"; rc=$?
if [ $rc -ne 0 ] && echo "$out" | grep -qi "identity mismatch"; then
    pass "assert_commit rejects a moved/mismatched commit"
else
    fail "assert_commit did not reject a mismatched commit (rc=$rc): $out"
fi
rm -rf "$DIR"

echo ""
echo "release_staging: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
