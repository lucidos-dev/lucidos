#!/usr/bin/env bash
# Tests for scripts/lib/headless_tarball.sh — the headless-tarball packaging
# helpers (step 1 of docs/plans/2026-06-29-installer-bundled-rework.md). Pure
# tar/gzip + python3 sha256, no git/gh/network, so the whole matrix runs offline
# with fake resource files. Run: ./scripts/lib/headless_tarball_test.sh
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/lib/release_staging.sh
source "$SCRIPT_DIR/release_staging.sh"   # release_staging_sha256
# shellcheck source=scripts/lib/headless_tarball.sh
source "$SCRIPT_DIR/headless_tarball.sh"

PASS=0
FAIL=0
fail() { echo "  FAIL: $*"; FAIL=$((FAIL+1)); }
pass() { echo "  ok:   $*"; PASS=$((PASS+1)); }

VERSION="0.14.0"
TRIPLE="aarch64-apple-darwin"
NAMES=(lucidos-engine lucidos-gateway lucidos frontend postgres sdk)

# A fake resources dir shaped like the real .app Contents/Resources: two loose
# "binaries", two static dirs, and a relocatable-PG-like nested tree with a
# symlink (to exercise ditto's symlink/dir handling).
new_resources() {
    local dir
    dir="$(mktemp -d)"
    printf 'engine\n'   > "$dir/lucidos-engine"
    printf 'gateway\n'  > "$dir/lucidos-gateway"
    printf 'cli\n'      > "$dir/lucidos"
    chmod +x "$dir/lucidos-engine" "$dir/lucidos-gateway" "$dir/lucidos"
    mkdir -p "$dir/frontend" && printf '<html>\n' > "$dir/frontend/index.html"
    mkdir -p "$dir/sdk"      && printf 'sdk\n'     > "$dir/sdk/sdk.js"
    mkdir -p "$dir/postgres/bin" "$dir/postgres/lib"
    printf 'postgres\n' > "$dir/postgres/bin/postgres"
    chmod +x "$dir/postgres/bin/postgres"
    printf 'libpq\n'    > "$dir/postgres/lib/libpq.5.dylib"
    ln -s libpq.5.dylib "$dir/postgres/lib/libpq.dylib"
    printf '%s' "$dir"
}

# ── stem naming ──────────────────────────────────────────────────────────────
echo "test: headless_tarball_stem builds lucidos-<version>-<triple>"
got="$(headless_tarball_stem "$VERSION" "$TRIPLE")"
[ "$got" = "lucidos-$VERSION-$TRIPLE" ] && pass "stem = $got" || fail "stem wrong: '$got'"

# ── happy path: tarball + sidecar + contents ─────────────────────────────────
echo ""
echo "test: emit produces a tarball + matching .sha256 sidecar"
RES="$(new_resources)"
OUT="$(mktemp -d)"
TARBALL="$(headless_tarball_emit "$RES" "$OUT" "$VERSION" "$TRIPLE" "${NAMES[@]}")"; rc=$?
if [ $rc -eq 0 ] && [ -n "$TARBALL" ]; then
    pass "emit exits 0 and prints a path"
else
    fail "emit failed (rc=$rc)"
fi
STEM="lucidos-$VERSION-$TRIPLE"
[ "$TARBALL" = "$OUT/$STEM.tar.gz" ] && pass "tarball path = $TARBALL" || fail "unexpected path: '$TARBALL'"
[ -f "$OUT/$STEM.tar.gz" ]        && pass "tarball exists"  || fail "tarball missing"
[ -f "$OUT/$STEM.tar.gz.sha256" ] && pass "sidecar exists"  || fail "sidecar missing"

echo ""
echo "test: tarball members live under one <stem>/ prefix with the 6 resources"
members="$(tar -tzf "$OUT/$STEM.tar.gz")"
for name in "${NAMES[@]}"; do
    if echo "$members" | grep -q "^$STEM/$name"; then
        pass "contains $STEM/$name"
    else
        fail "missing $STEM/$name in: $members"
    fi
done
# The nested PG tree + symlink must come through.
echo "$members" | grep -q "^$STEM/postgres/bin/postgres$" \
    && pass "contains nested postgres/bin/postgres" || fail "missing nested postgres binary"

echo ""
echo "test: no AppleDouble (._*) members in the tarball"
if echo "$members" | grep -q '/\._'; then
    fail "tarball carries ._ AppleDouble members: $members"
else
    pass "no ._ AppleDouble members"
fi

echo ""
echo "test: sidecar is shasum -a 256 -c verifiable and basename-relative"
# Sidecar must reference the basename (not an absolute path) so it verifies from
# the artifact dir — exactly how install.sh (step 3) will check a downloaded file.
if grep -q "  $STEM.tar.gz\$" "$OUT/$STEM.tar.gz.sha256"; then
    pass "sidecar names the tarball by basename"
else
    fail "sidecar does not reference the basename: $(cat "$OUT/$STEM.tar.gz.sha256")"
fi
if ( cd "$OUT" && shasum -a 256 -c "$STEM.tar.gz.sha256" >/dev/null 2>&1 ); then
    pass "shasum -a 256 -c passes"
else
    fail "shasum -a 256 -c failed"
fi
# Tamper → checksum must no longer verify.
printf 'tampered' >> "$OUT/$STEM.tar.gz"
if ( cd "$OUT" && shasum -a 256 -c "$STEM.tar.gz.sha256" >/dev/null 2>&1 ); then
    fail "shasum -c accepted a tampered tarball"
else
    pass "shasum -c rejects a tampered tarball"
fi
rm -rf "$RES" "$OUT"

# ── failure paths ────────────────────────────────────────────────────────────
echo ""
echo "test: emit refuses a missing resource"
RES="$(new_resources)"; OUT="$(mktemp -d)"
rm -rf "$RES/sdk"
out="$(headless_tarball_emit "$RES" "$OUT" "$VERSION" "$TRIPLE" "${NAMES[@]}" 2>&1)"; rc=$?
if [ $rc -ne 0 ] && echo "$out" | grep -qi "missing"; then
    pass "missing resource is refused"
else
    fail "expected missing-resource refusal (rc=$rc): $out"
fi
[ -f "$OUT/$STEM.tar.gz" ] && fail "tarball should not exist after a refusal" || pass "no tarball written on refusal"
rm -rf "$RES" "$OUT"

echo ""
echo "test: emit refuses a non-existent resources dir"
OUT="$(mktemp -d)"
out="$(headless_tarball_emit "/no/such/resources/dir" "$OUT" "$VERSION" "$TRIPLE" "${NAMES[@]}" 2>&1)"; rc=$?
if [ $rc -ne 0 ] && echo "$out" | grep -qi "does not exist"; then
    pass "missing resources dir is refused"
else
    fail "expected missing-dir refusal (rc=$rc): $out"
fi
rm -rf "$OUT"

echo ""
echo "test: emit refuses an empty resource-name list"
RES="$(new_resources)"; OUT="$(mktemp -d)"
out="$(headless_tarball_emit "$RES" "$OUT" "$VERSION" "$TRIPLE" 2>&1)"; rc=$?
if [ $rc -ne 0 ] && echo "$out" | grep -qi "at least one resource name"; then
    pass "empty name list is refused"
else
    fail "expected empty-name-list refusal (rc=$rc): $out"
fi
rm -rf "$RES" "$OUT"

echo ""
echo "headless_tarball: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
