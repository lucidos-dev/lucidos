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
if [ -f "$DIR/manifest.json" ]; then pass "manifest.json was created"; else fail "manifest.json missing"; fi

got_ver="$(release_staging_manifest_field "$DIR" version 2>/dev/null)"
if [ "$got_ver" = "$VERSION" ]; then pass "version field = $VERSION"; else fail "version field wrong: '$got_ver'"; fi

got_commit="$(release_staging_manifest_field "$DIR" source_commit 2>/dev/null)"
if [ "$got_commit" = "$COMMIT" ]; then pass "source_commit field = $COMMIT"; else fail "source_commit wrong: '$got_commit'"; fi

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

# ── fingerprint fields: recorded when supplied, absent when not ──────────────
# The compiled-input gate (release_build_fingerprint.sh) stores what a staged
# artifact was built from in CONTENT terms, so a later re-fold can skip a
# rebuild + a redundant Apple notarization when nothing shipped would change.
echo "test: manifest carries build/recipe fingerprints when supplied"
DIR="$(new_staging)"
RELEASE_STAGING_BUILD_FINGERPRINT="v1:abc123" \
RELEASE_STAGING_RECIPE_FINGERPRINT="v1:def456" \
release_staging_write_manifest "$DIR" "$VERSION" "$COMMIT" \
    "Lucidos_0.12.2_aarch64.dmg" "Lucidos.app.tar.gz" "Lucidos.app.tar.gz.sig" >/dev/null
GOT="$(release_staging_manifest_field "$DIR" build_fingerprint)"
if [ "$GOT" = "v1:abc123" ]; then
    pass "build_fingerprint round-trips"
else
    fail "build_fingerprint was '$GOT'"
fi
GOT="$(release_staging_manifest_field "$DIR" recipe_fingerprint)"
if [ "$GOT" = "v1:def456" ]; then
    pass "recipe_fingerprint round-trips"
else
    fail "recipe_fingerprint was '$GOT'"
fi
if release_staging_verify "$DIR"; then
    pass "verify still passes with the new fields"
else
    fail "verify broke on a fingerprinted manifest"
fi
rm -rf "$DIR"

echo "test: fingerprints are OMITTED (not empty) when not supplied"
DIR="$(new_staging)"
release_staging_write_manifest "$DIR" "$VERSION" "$COMMIT" \
    "Lucidos_0.12.2_aarch64.dmg" "Lucidos.app.tar.gz" "Lucidos.app.tar.gz.sig" >/dev/null
if grep -q 'build_fingerprint' "$DIR/manifest.json"; then
    fail "an unsupplied fingerprint was written into the manifest"
else
    pass "no fingerprint key when none was supplied (absent ≠ empty)"
fi
if release_staging_manifest_field "$DIR" build_fingerprint >/dev/null 2>&1; then
    fail "a missing fingerprint read as present without --optional"
else
    pass "a missing fingerprint is an error without --optional"
fi
GOT="$(release_staging_manifest_field "$DIR" build_fingerprint --optional)"
if [ -z "$GOT" ]; then
    pass "--optional yields empty + success for a legacy manifest"
else
    fail "--optional returned '$GOT'"
fi
if release_staging_verify "$DIR"; then
    pass "a legacy (fingerprint-less) manifest still verifies"
else
    fail "verify rejected a legacy manifest"
fi
rm -rf "$DIR"

# ── notarized: the deferred-DMG flag ──────────────────────────────────────────
# The field every public-facing consumer reads to decide whether a release ships
# with the "notarization pending" banner. Its back-compat direction is
# load-bearing: manifests written before the deferred mode existed have no key,
# and that writer staged only after an Accepted verdict — so absent MUST read as
# notarized, while a corrupt/missing manifest must read as NOT notarized so the
# degenerate case errs toward the banner.
echo ""
echo "test: notarized=false round-trips and reads as not-notarized"
DIR="$(new_staging)"
RELEASE_STAGING_NOTARIZED=false release_staging_write_manifest "$DIR" "$VERSION" "$COMMIT" \
    "Lucidos_0.12.2_aarch64.dmg" "Lucidos.app.tar.gz" "Lucidos.app.tar.gz.sig" >/dev/null
GOT="$(release_staging_manifest_field "$DIR" notarized)"
if [ "$GOT" = "false" ]; then
    pass "a JSON boolean reads back as the shell-comparable 'false'"
else
    fail "notarized read back as '$GOT' (Python's str(False) would be 'False')"
fi
if release_staging_is_notarized "$DIR"; then
    fail "is_notarized accepted a deferred staging"
else
    pass "is_notarized is non-zero for notarized=false"
fi
if release_staging_verify "$DIR"; then
    pass "a deferred staging still verifies (the flag is orthogonal to integrity)"
else
    fail "verify rejected a staging carrying notarized=false"
fi
rm -rf "$DIR"

echo ""
echo "test: notarized=true round-trips and reads as notarized"
DIR="$(new_staging)"
RELEASE_STAGING_NOTARIZED=true release_staging_write_manifest "$DIR" "$VERSION" "$COMMIT" \
    "Lucidos_0.12.2_aarch64.dmg" "Lucidos.app.tar.gz" "Lucidos.app.tar.gz.sig" >/dev/null
GOT="$(release_staging_manifest_field "$DIR" notarized)"
if [ "$GOT" = "true" ]; then pass "notarized reads back as 'true'"; else fail "notarized read back as '$GOT'"; fi
if release_staging_is_notarized "$DIR"; then
    pass "is_notarized is zero for notarized=true"
else
    fail "is_notarized rejected a notarized staging"
fi
rm -rf "$DIR"

echo ""
echo "test: an absent notarized key reads as NOTARIZED (pre-deferred-mode manifests)"
DIR="$(new_staging)"
release_staging_write_manifest "$DIR" "$VERSION" "$COMMIT" \
    "Lucidos_0.12.2_aarch64.dmg" "Lucidos.app.tar.gz" "Lucidos.app.tar.gz.sig" >/dev/null
if grep -q 'notarized' "$DIR/manifest.json"; then
    fail "an unsupplied notarized flag was written into the manifest"
else
    pass "no notarized key when none was supplied (absent ≠ false)"
fi
if release_staging_is_notarized "$DIR"; then
    pass "a legacy manifest reads as notarized"
else
    fail "a legacy manifest read as pending — every past release would re-banner"
fi
rm -rf "$DIR"

echo ""
echo "test: a non-boolean notarized value is refused, not coerced"
DIR="$(new_staging)"
RELEASE_STAGING_NOTARIZED=yes release_staging_write_manifest "$DIR" "$VERSION" "$COMMIT" \
    "Lucidos_0.12.2_aarch64.dmg" "Lucidos.app.tar.gz" "Lucidos.app.tar.gz.sig" >/dev/null
if grep -q 'notarized' "$DIR/manifest.json"; then
    fail "a typo'd value ('yes') was written as a notarization state"
else
    pass "only the literal true/false spellings are honoured"
fi
rm -rf "$DIR"

echo ""
echo "test: a missing manifest reads as NOT notarized (fail closed)"
DIR="$(mktemp -d)"
if release_staging_is_notarized "$DIR"; then
    fail "is_notarized returned zero for a dir with no manifest"
else
    pass "no manifest ⇒ not notarized, so a corrupt staging errs toward the banner"
fi
rm -rf "$DIR"

echo ""
echo "release_staging: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
