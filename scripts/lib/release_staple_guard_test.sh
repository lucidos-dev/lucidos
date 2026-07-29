#!/usr/bin/env bash
# Tests for the staple-time byte assertion in scripts/build-dmg.sh —
# assert_dmg_is_the_submitted_bytes() and notarize_pin_submitted_dmg().
#
# THE BUG THESE PIN DOWN (2026-07-28). build-dmg.sh writes the DMG to a FIXED
# path, so a rebuild overwrites the exact file an in-flight notarization was
# submitted for. That day three pollers ran concurrently and two were waiting on
# submissions whose bytes had already been overwritten; had those verdicts
# returned, each would have stapled a ticket issued for one set of bytes onto a
# different set. The resume path had a checksum gate; the fresh-build path had
# none, despite having the identical submit → long wait → staple window.
#
# build-dmg.sh is a script, not a library, so the two functions are extracted
# with awk (the same technique build_dmg_test.sh already uses to test pieces of
# it) and exercised against fake DMG files. No xcrun, no network, no build.
# Run: ./scripts/lib/release_staple_guard_test.sh
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
# shellcheck source=scripts/lib/release_staging.sh
source "$SCRIPT_DIR/release_staging.sh"

PASS=0
FAIL=0
fail() { echo "  FAIL: $*"; FAIL=$((FAIL+1)); }
pass() { echo "  ok:   $*"; PASS=$((PASS+1)); }

# Extract the two functions under test from build-dmg.sh.
EXTRACT="$(mktemp)"
awk '/^assert_dmg_is_the_submitted_bytes\(\) \{/,/^\}/'  "$PROJECT_DIR/scripts/build-dmg.sh" >  "$EXTRACT"
awk '/^notarize_pin_submitted_dmg\(\) \{/,/^\}/'         "$PROJECT_DIR/scripts/build-dmg.sh" >> "$EXTRACT"
if [ ! -s "$EXTRACT" ]; then
    echo "  FAIL: could not extract the guard functions from build-dmg.sh"
    exit 1
fi
# `die` and `step` come from build-dmg.sh's own preamble; stub them.
die()  { echo "DIE: $*" >&2; return 1; }
step() { :; }
# shellcheck source=/dev/null
source "$EXTRACT"
rm -f "$EXTRACT"

new_tree() {
    local root
    root="$(mktemp -d)"
    mkdir -p "$root/target/release/bundle/dmg"
    printf 'the signed dmg bytes\n' > "$root/target/release/bundle/dmg/Lucidos_0.77.0_aarch64.dmg"
    printf '%s' "$root"
}

# ── 1. unchanged bytes ⇒ the staple proceeds ─────────────────────────────────
echo "test: an unchanged DMG passes the staple-time assertion"
ROOT="$(new_tree)"
# shellcheck disable=SC2034 # global read by notarize_pin_submitted_dmg
REPO_ROOT="$ROOT"
# shellcheck disable=SC2034 # global read by notarize_pin_submitted_dmg
EFFECTIVE_VERSION="0.77.0"
DMG_PATH="$ROOT/target/release/bundle/dmg/Lucidos_0.77.0_aarch64.dmg"
NOTARIZE_PINNED_DMG=""
SHA="$(release_staging_sha256 "$DMG_PATH")"
if assert_dmg_is_the_submitted_bytes "$SHA" 2>/dev/null; then
    pass "identical bytes ⇒ assertion passes"
else
    fail "the assertion rejected unchanged bytes"
fi
rm -rf "$ROOT"

# ── 2. THE INCIDENT: overwritten bytes, no pin ⇒ REFUSE to staple ────────────
echo "test: a DMG overwritten by a rebuild is REFUSED (no ticket on wrong bytes)"
ROOT="$(new_tree)"
# shellcheck disable=SC2034 # global read by notarize_pin_submitted_dmg
REPO_ROOT="$ROOT"
# shellcheck disable=SC2034 # global read by notarize_pin_submitted_dmg
EFFECTIVE_VERSION="0.77.0"
DMG_PATH="$ROOT/target/release/bundle/dmg/Lucidos_0.77.0_aarch64.dmg"
NOTARIZE_PINNED_DMG=""
SHA="$(release_staging_sha256 "$DMG_PATH")"
printf 'DIFFERENT bytes from a later rebuild\n' > "$DMG_PATH"   # the rebuild
OUT="$(assert_dmg_is_the_submitted_bytes "$SHA" 2>&1)"; RC=$?
if [ $RC -ne 0 ]; then
    pass "overwritten bytes ⇒ refuses (rc=$RC)"
else
    fail "STAPLED ONTO WRONG BYTES — the assertion passed after an overwrite"
fi
case "$OUT" in
    *"REFUSING TO STAPLE"*) pass "the refusal is explicit about why" ;;
    *)                      fail "unclear refusal: '$OUT'" ;;
esac
rm -rf "$ROOT"

# ── 3. a vanished DMG is refused, not silently skipped ───────────────────────
echo "test: a DMG deleted mid-flight is refused"
ROOT="$(new_tree)"
# shellcheck disable=SC2034 # global read by notarize_pin_submitted_dmg
REPO_ROOT="$ROOT"
# shellcheck disable=SC2034 # global read by notarize_pin_submitted_dmg
EFFECTIVE_VERSION="0.77.0"
DMG_PATH="$ROOT/target/release/bundle/dmg/Lucidos_0.77.0_aarch64.dmg"
NOTARIZE_PINNED_DMG=""
SHA="$(release_staging_sha256 "$DMG_PATH")"
rm -f "$DMG_PATH"
if assert_dmg_is_the_submitted_bytes "$SHA" 2>/dev/null; then
    fail "a missing DMG passed the assertion"
else
    pass "a vanished DMG ⇒ refuses"
fi
rm -rf "$ROOT"

# ── 4. an empty expectation is a no-op (unsigned/local builds) ───────────────
echo "test: no recorded sha ⇒ the assertion is a no-op (local unsigned builds)"
ROOT="$(new_tree)"
# shellcheck disable=SC2034 # global read by notarize_pin_submitted_dmg
REPO_ROOT="$ROOT"
# shellcheck disable=SC2034 # global read by notarize_pin_submitted_dmg
EFFECTIVE_VERSION="0.77.0"
DMG_PATH="$ROOT/target/release/bundle/dmg/Lucidos_0.77.0_aarch64.dmg"
NOTARIZE_PINNED_DMG=""
if assert_dmg_is_the_submitted_bytes "" 2>/dev/null; then
    pass "empty expectation ⇒ no-op"
else
    fail "an empty expectation was treated as a mismatch"
fi
rm -rf "$ROOT"

# ── 5. the pin keeps the submitted bytes reachable across a rebuild ──────────
echo "test: pinning survives a rebuild and RECOVERS the notarized bytes"
ROOT="$(new_tree)"
# shellcheck disable=SC2034 # global read by notarize_pin_submitted_dmg
REPO_ROOT="$ROOT"
# shellcheck disable=SC2034 # global read by notarize_pin_submitted_dmg
EFFECTIVE_VERSION="0.77.0"
DMG_PATH="$ROOT/target/release/bundle/dmg/Lucidos_0.77.0_aarch64.dmg"
NOTARIZE_PINNED_DMG=""
SHA="$(release_staging_sha256 "$DMG_PATH")"
notarize_pin_submitted_dmg "$SHA" >/dev/null
if [ -n "$NOTARIZE_PINNED_DMG" ] && [ -f "$NOTARIZE_PINNED_DMG" ]; then
    pass "pin created at $(basename "$(dirname "$NOTARIZE_PINNED_DMG")")/"
else
    fail "no pin was created"
fi
# A rebuild replaces the fixed path (mv-over, as hdiutil/tauri do).
printf 'DIFFERENT bytes from a later rebuild\n' > "$ROOT/newbuild.dmg"
mv -f "$ROOT/newbuild.dmg" "$DMG_PATH"
if [ "$(release_staging_sha256 "$NOTARIZE_PINNED_DMG")" = "$SHA" ]; then
    pass "the pinned copy still holds the submitted bytes after the rebuild"
else
    fail "the rebuild corrupted the pinned bytes"
fi
if assert_dmg_is_the_submitted_bytes "$SHA" 2>/dev/null; then
    pass "the assertion RECOVERS from the pin instead of failing the release"
else
    fail "recovery from the pin did not happen"
fi
if [ "$(release_staging_sha256 "$DMG_PATH")" = "$SHA" ]; then
    pass "the fixed path now holds the notarized bytes again"
else
    fail "the fixed path was not restored"
fi
rm -rf "$ROOT"

# ── 5b. THE HARDLINK HAZARD: an IN-PLACE write must not reach the pin ────────
# A hardlink pin is a second name for the same inode, so `codesign` rewriting the
# signature in place — or any truncating write — silently corrupts the pinned
# bytes and the recovery path hands back exactly the wrong thing it exists to
# prevent. A clone (cp -c) is copy-on-write, so the original diverges alone.
echo "test: an IN-PLACE write to the DMG does not reach the pinned copy"
ROOT="$(new_tree)"
# shellcheck disable=SC2034 # global read by notarize_pin_submitted_dmg
REPO_ROOT="$ROOT"
# shellcheck disable=SC2034 # global read by notarize_pin_submitted_dmg
EFFECTIVE_VERSION="0.77.0"
DMG_PATH="$ROOT/target/release/bundle/dmg/Lucidos_0.77.0_aarch64.dmg"
NOTARIZE_PINNED_DMG=""
SHA="$(release_staging_sha256 "$DMG_PATH")"
notarize_pin_submitted_dmg "$SHA" >/dev/null
PIN="$NOTARIZE_PINNED_DMG"
# Truncate-in-place (`>`), the shape `codesign` and friends use — NOT mv-over.
printf 're-signed in place
' > "$DMG_PATH"
if [ "$(release_staging_sha256 "$PIN")" = "$SHA" ]; then
    pass "the pin survived an in-place write (clone, not hardlink)"
else
    fail "an in-place write corrupted the pin — the pin shares the inode"
fi
rm -rf "$ROOT"

# ── 6. recovery works from the on-disk pin dir alone (fresh process) ─────────
# A resumed poller is a NEW process: NOTARIZE_PINNED_DMG is empty, so recovery
# must find the pin by content address. This is the orphaned-poller case.
echo "test: a fresh process finds the pin by content address"
ROOT="$(new_tree)"
# shellcheck disable=SC2034 # global read by notarize_pin_submitted_dmg
REPO_ROOT="$ROOT"
# shellcheck disable=SC2034 # global read by notarize_pin_submitted_dmg
EFFECTIVE_VERSION="0.77.0"
DMG_PATH="$ROOT/target/release/bundle/dmg/Lucidos_0.77.0_aarch64.dmg"
NOTARIZE_PINNED_DMG=""
SHA="$(release_staging_sha256 "$DMG_PATH")"
notarize_pin_submitted_dmg "$SHA" >/dev/null
printf 'rebuild\n' > "$DMG_PATH"
NOTARIZE_PINNED_DMG=""   # a fresh process knows nothing
if assert_dmg_is_the_submitted_bytes "$SHA" 2>/dev/null; then
    pass "found the pin via .lucidos/notarize-submissions/<version>/<sha12>/"
else
    fail "a fresh process could not locate the pinned bytes"
fi
rm -rf "$ROOT"

# ── 7. two different submissions pin to two different dirs ───────────────────
echo "test: concurrent submissions do not collide (content-addressed pins)"
ROOT="$(new_tree)"
# shellcheck disable=SC2034 # global read by notarize_pin_submitted_dmg
REPO_ROOT="$ROOT"
# shellcheck disable=SC2034 # global read by notarize_pin_submitted_dmg
EFFECTIVE_VERSION="0.77.0"
DMG_PATH="$ROOT/target/release/bundle/dmg/Lucidos_0.77.0_aarch64.dmg"
NOTARIZE_PINNED_DMG=""
SHA_A="$(release_staging_sha256 "$DMG_PATH")"
notarize_pin_submitted_dmg "$SHA_A" >/dev/null
PIN_A="$NOTARIZE_PINNED_DMG"
printf 'second build bytes\n' > "$DMG_PATH"   # in-place, the hostile case
NOTARIZE_PINNED_DMG=""
SHA_B="$(release_staging_sha256 "$DMG_PATH")"
notarize_pin_submitted_dmg "$SHA_B" >/dev/null
PIN_B="$NOTARIZE_PINNED_DMG"
if [ "$PIN_A" != "$PIN_B" ] && [ -f "$PIN_A" ] && [ -f "$PIN_B" ]; then
    pass "two in-flight submissions keep two distinct pinned copies"
else
    fail "the second pin collided with the first ($PIN_A vs $PIN_B)"
fi
if [ "$(release_staging_sha256 "$PIN_A")" = "$SHA_A" ] \
&& [ "$(release_staging_sha256 "$PIN_B")" = "$SHA_B" ]; then
    pass "each pin holds its own submission's bytes"
else
    fail "a pin holds the wrong bytes"
fi
rm -rf "$ROOT"

echo ""
echo "release_staple_guard: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
