#!/usr/bin/env bash
# Tests for the staple-time byte assertion in scripts/build-dmg.sh: the pin
# helpers (notarize_pin_dir / notarize_pin_artifact / notarize_find_pin /
# notarize_pin_submitted_set) and the set assertion
# (assert_submitted_artifacts_are_intact / assert_submitted_set_is_intact).
#
# THE FIRST BUG THESE PIN DOWN (2026-07-28). build-dmg.sh writes the DMG to a
# FIXED path, so a rebuild overwrites the exact file an in-flight notarization was
# submitted for. That day three pollers ran concurrently and two were waiting on
# submissions whose bytes had already been overwritten; had those verdicts
# returned, each would have stapled a ticket issued for one set of bytes onto a
# different set. The resume path had a checksum gate; the fresh-build path had
# none, despite the identical submit, long wait, staple window.
#
# THE SECOND (F3, 2026-08-02). That guard covered the DMG and nothing else, and a
# release ships three files that must come from ONE build. The recovery branch
# made it active rather than passive: it restores the DMG from its pin after a
# concurrent rebuild, which is exactly the state in which the tarball beside it
# belongs to the NEWER build. So the assertion now decides over the whole set
# before it copies anything, and the "half a build" case below is the one that
# would previously have sailed through.
#
# THE THIRD (v0.19.1 Phase A, 2026-08-02). The guard could not tell OUR OWN
# STAPLE from a concurrent rebuild. `xcrun stapler staple` writes the ticket INTO
# the DMG, so the bytes change by design; staging's second assertion then read
# that as a rebuild, restored the pre-staple pin over the stapled image and
# silently undid the staple. The manifest recorded the unstapled sha, so
# everything downstream was self-consistent and `--publish-verified` would have
# shipped a DMG with no ticket. Section 9 is that sequence end to end, asserting
# the bytes and the manifest rather than any log line, plus the two cases that
# prove the guard keeps its force past the staple.
#
# build-dmg.sh is a script, not a library, so the functions are extracted with awk
# (the same technique build_dmg_test.sh already uses) and exercised against fake
# files. No xcrun, no network, no build.
# Run: ./scripts/lib/release_staple_guard_test.sh
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
# shellcheck source=scripts/lib/release_staging.sh
source "$SCRIPT_DIR/release_staging.sh"
# The resume handle is the THIRD place the expected bytes live, and section 9
# asserts it moves with the other two. Sourced after release_staging.sh, which it
# depends on for release_staging_sha256.
# shellcheck source=scripts/lib/release_notarize.sh
source "$SCRIPT_DIR/release_notarize.sh"

PASS=0
FAIL=0
fail() { echo "  FAIL: $*"; FAIL=$((FAIL+1)); }
pass() { echo "  ok:   $*"; PASS=$((PASS+1)); }

# Extract the functions under test from build-dmg.sh.
EXTRACT="$(mktemp)"
for fn in notarize_pin_dir notarize_pin_artifact notarize_find_pin \
          notarize_pin_submitted_set assert_submitted_artifacts_are_intact \
          assert_submitted_set_is_intact staple_idempotent dmg_ticket_is_stapled \
          notarize_carry_staple_into_handle notarize_record_stapled_dmg \
          staple_notarized_artifacts; do
    awk -v fn="$fn" '$0 ~ "^" fn "\\(\\) \\{",/^\}/' "$PROJECT_DIR/scripts/build-dmg.sh" >> "$EXTRACT"
    if ! grep -q "^$fn() {" "$EXTRACT"; then
        echo "  FAIL: could not extract $fn from build-dmg.sh"
        exit 1
    fi
done
# `die` and `step` come from build-dmg.sh's own preamble; stub them.
die()  { echo "DIE: $*" >&2; return 1; }
step() { :; }
# shellcheck source=/dev/null
source "$EXTRACT"
rm -f "$EXTRACT"

VERSION="0.77.0"   # synthetic: a fixture must never collide with a real release

# A stand-in for `xcrun stapler`, and the whole point of it is that `staple`
# MUTATES THE FILE the way the real one does: the ticket is written into the
# image, so its bytes change. A fake that only printed "worked!" could not
# express the bug section 9 pins down.
FAKE_BIN="$(mktemp -d)"
cat > "$FAKE_BIN/xcrun" <<'FAKE'
#!/usr/bin/env bash
[ "$1" = "stapler" ] || { echo "fake xcrun: only stapler is faked, got '$1'" >&2; exit 1; }
case "$2" in
    staple)
        grep -q 'NOTARIZATION-TICKET' "$3" 2>/dev/null && exit 1   # already stapled
        printf 'NOTARIZATION-TICKET\n' >> "$3"
        echo "The staple and validate action worked!" ;;
    validate)
        # A toolchain failure is a DIFFERENT answer from "no ticket", and the
        # code has to tell them apart, so the fake can produce both. Any other
        # exit stands in for no xcrun / no selected developer dir.
        grep -q 'BROKEN-TOOLCHAIN' "$3" 2>/dev/null && {
            echo "xcode-select: error: tool 'stapler' requires Xcode" >&2; exit 127; }
        # Exit 65 with no ticket, which is what the rc DMG-verify leg reported.
        grep -q 'NOTARIZATION-TICKET' "$3" 2>/dev/null || exit 65 ;;
    *)  echo "fake xcrun: unknown stapler verb '$2'" >&2; exit 1 ;;
esac
FAKE
chmod +x "$FAKE_BIN/xcrun"
PATH="$FAKE_BIN:$PATH"

has_ticket() { xcrun stapler validate "$1" >/dev/null 2>&1; }

# The sha256 the staging manifest recorded for one artifact.
manifest_sha() {  # <staging-dir> <artifact-name>
    NAME="$2" python3 - "$1/manifest.json" <<'PY'
import json, os, sys
with open(sys.argv[1], encoding="utf-8") as f:
    manifest = json.load(f)
for artifact in manifest.get("artifacts", []):
    if artifact["name"] == os.environ["NAME"]:
        print(artifact["sha256"])
        break
PY
}

# A tree holding the three artifacts a release submits and ships, with the
# build-dmg.sh globals the extracted functions read pointed at them.
new_tree() {
    local root
    root="$(mktemp -d)"
    mkdir -p "$root/target/release/bundle/dmg" "$root/target/release/bundle/macos"
    printf 'the signed dmg bytes\n'   > "$root/target/release/bundle/dmg/Lucidos_${VERSION}_aarch64.dmg"
    printf 'the updater payload\n'    > "$root/target/release/bundle/macos/Lucidos.app.tar.gz"
    printf 'the updater signature\n'  > "$root/target/release/bundle/macos/Lucidos.app.tar.gz.sig"
    printf '%s' "$root"
}

# shellcheck disable=SC2034 # every global here is read by an extracted function
use_tree() {  # <root>  [with-pairing]
    ROOT="$1"
    REPO_ROOT="$ROOT"
    EFFECTIVE_VERSION="$VERSION"
    DMG_PATH="$ROOT/target/release/bundle/dmg/Lucidos_${VERSION}_aarch64.dmg"
    DMG_SHA="$(release_staging_sha256 "$DMG_PATH")"
    if [ "${2:-}" = "with-pairing" ]; then
        NOTARIZE_UPDATER_TARBALL="$ROOT/target/release/bundle/macos/Lucidos.app.tar.gz"
        NOTARIZE_UPDATER_TARBALL_SHA="$(release_staging_sha256 "$NOTARIZE_UPDATER_TARBALL")"
        NOTARIZE_UPDATER_SIG_SHA="$(release_staging_sha256 "$NOTARIZE_UPDATER_TARBALL.sig")"
    else
        NOTARIZE_UPDATER_TARBALL=""
        NOTARIZE_UPDATER_TARBALL_SHA=""
        NOTARIZE_UPDATER_SIG_SHA=""
    fi
}

# ── 1. unchanged bytes ⇒ the staple proceeds ─────────────────────────────────
echo "test: an unchanged set passes the staple-time assertion"
use_tree "$(new_tree)" with-pairing
if assert_submitted_set_is_intact "$DMG_SHA" 2>/dev/null; then
    pass "identical bytes ⇒ assertion passes"
else
    fail "the assertion rejected an unchanged set"
fi
rm -rf "$ROOT"

# ── 2. THE INCIDENT: overwritten bytes, no pin ⇒ REFUSE to staple ────────────
echo ""
echo "test: a DMG overwritten by a rebuild is REFUSED (no ticket on wrong bytes)"
use_tree "$(new_tree)"
printf 'DIFFERENT bytes from a later rebuild\n' > "$DMG_PATH"   # the rebuild
OUT="$(assert_submitted_set_is_intact "$DMG_SHA" 2>&1)"; RC=$?
if [ $RC -ne 0 ]; then
    pass "overwritten bytes ⇒ refuses (rc=$RC)"
else
    fail "STAPLED ONTO WRONG BYTES: the assertion passed after an overwrite"
fi
case "$OUT" in
    *"REFUSING TO STAPLE"*) pass "the refusal is explicit about why" ;;
    *)                      fail "unclear refusal: '$OUT'" ;;
esac
rm -rf "$ROOT"

# ── 3. a vanished DMG is refused, not silently skipped ───────────────────────
echo ""
echo "test: a DMG deleted mid-flight is refused"
use_tree "$(new_tree)"
rm -f "$DMG_PATH"
if assert_submitted_set_is_intact "$DMG_SHA" 2>/dev/null; then
    fail "a missing DMG passed the assertion"
else
    pass "a vanished DMG ⇒ refuses"
fi
rm -rf "$ROOT"

# ── 4. an empty expectation is a no-op (unsigned/local builds) ───────────────
echo ""
echo "test: no recorded sha ⇒ the assertion is a no-op (local unsigned builds)"
use_tree "$(new_tree)"
if assert_submitted_set_is_intact "" 2>/dev/null; then
    pass "empty expectation ⇒ no-op"
else
    fail "an empty expectation was treated as a mismatch"
fi
rm -rf "$ROOT"

# ── 5. the pin keeps the submitted bytes reachable across a rebuild ──────────
echo ""
echo "test: pinning survives a rebuild and RECOVERS the notarized bytes"
use_tree "$(new_tree)"
notarize_pin_submitted_set "$DMG_PATH" "$DMG_SHA" >/dev/null
PIN="$(notarize_find_pin "$DMG_SHA")"
if [ -n "$PIN" ] && [ -f "$PIN" ]; then
    pass "pin created at $(basename "$(dirname "$PIN")")/"
else
    fail "no pin was created"
fi
# A rebuild replaces the fixed path (mv-over, as hdiutil/tauri do).
printf 'DIFFERENT bytes from a later rebuild\n' > "$ROOT/newbuild.dmg"
mv -f "$ROOT/newbuild.dmg" "$DMG_PATH"
if [ "$(release_staging_sha256 "$PIN")" = "$DMG_SHA" ]; then
    pass "the pinned copy still holds the submitted bytes after the rebuild"
else
    fail "the rebuild corrupted the pinned bytes"
fi
if assert_submitted_set_is_intact "$DMG_SHA" 2>/dev/null; then
    pass "the assertion RECOVERS from the pin instead of failing the release"
else
    fail "recovery from the pin did not happen"
fi
if [ "$(release_staging_sha256 "$DMG_PATH")" = "$DMG_SHA" ]; then
    pass "the fixed path now holds the notarized bytes again"
else
    fail "the fixed path was not restored"
fi
rm -rf "$ROOT"

# ── 5b. THE HARDLINK HAZARD: an IN-PLACE write must not reach the pin ────────
# A hardlink pin is a second name for the same inode, so `codesign` rewriting the
# signature in place, or any truncating write, silently corrupts the pinned bytes
# and the recovery path hands back exactly the wrong thing it exists to prevent.
# A clone (cp -c) is copy-on-write, so the original diverges alone.
echo ""
echo "test: an IN-PLACE write to the DMG does not reach the pinned copy"
use_tree "$(new_tree)"
notarize_pin_submitted_set "$DMG_PATH" "$DMG_SHA" >/dev/null
PIN="$(notarize_find_pin "$DMG_SHA")"
# Truncate-in-place (`>`), the shape `codesign` and friends use, NOT mv-over.
printf 're-signed in place\n' > "$DMG_PATH"
if [ "$(release_staging_sha256 "$PIN")" = "$DMG_SHA" ]; then
    pass "the pin survived an in-place write (clone, not hardlink)"
else
    fail "an in-place write corrupted the pin: the pin shares the inode"
fi
rm -rf "$ROOT"

# ── 6. recovery works from the on-disk pin dir alone (fresh process) ─────────
# A resumed poller is a NEW process and knows nothing about what the dead one
# pinned, so recovery must find the pin by content address. This is the
# orphaned-poller case, and it is why there is no "the pin I just made" global.
echo ""
echo "test: a fresh process finds the pin by content address"
use_tree "$(new_tree)"
notarize_pin_submitted_set "$DMG_PATH" "$DMG_SHA" >/dev/null
printf 'rebuild\n' > "$DMG_PATH"
if assert_submitted_set_is_intact "$DMG_SHA" 2>/dev/null; then
    pass "found the pin via .lucidos/notarize-submissions/<version>/<sha12>/"
else
    fail "a fresh process could not locate the pinned bytes"
fi
rm -rf "$ROOT"

# ── 7. two different submissions pin to two different dirs ───────────────────
echo ""
echo "test: concurrent submissions do not collide (content-addressed pins)"
use_tree "$(new_tree)"
SHA_A="$DMG_SHA"
notarize_pin_submitted_set "$DMG_PATH" "$SHA_A" >/dev/null
PIN_A="$(notarize_find_pin "$SHA_A")"
printf 'second build bytes\n' > "$DMG_PATH"   # in-place, the hostile case
SHA_B="$(release_staging_sha256 "$DMG_PATH")"
notarize_pin_submitted_set "$DMG_PATH" "$SHA_B" >/dev/null
PIN_B="$(notarize_find_pin "$SHA_B")"
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

# ── 8. F3: THE SET IS PINNED AND RECOVERED TOGETHER ──────────────────────────
echo ""
echo "test: the whole submitted set is pinned, not just the DMG"
use_tree "$(new_tree)" with-pairing
notarize_pin_submitted_set "$DMG_PATH" "$DMG_SHA" >/dev/null
for label_sha in "dmg:$DMG_SHA" "tarball:$NOTARIZE_UPDATER_TARBALL_SHA" "sig:$NOTARIZE_UPDATER_SIG_SHA"; do
    if [ -n "$(notarize_find_pin "${label_sha#*:}")" ]; then
        pass "the ${label_sha%%:*} is pinned"
    else
        fail "the ${label_sha%%:*} was not pinned"
    fi
done
rm -rf "$ROOT"

echo ""
echo "test: a rebuild that replaced the WHOLE set is recovered in full"
use_tree "$(new_tree)" with-pairing
notarize_pin_submitted_set "$DMG_PATH" "$DMG_SHA" >/dev/null
printf 'newer dmg\n'     > "$DMG_PATH"
printf 'newer tarball\n' > "$NOTARIZE_UPDATER_TARBALL"
printf 'newer sig\n'     > "$NOTARIZE_UPDATER_TARBALL.sig"
if assert_submitted_set_is_intact "$DMG_SHA" >/dev/null 2>&1; then
    pass "a fully pinned set is recovered"
else
    fail "a fully pinned set was refused"
fi
if [ "$(release_staging_sha256 "$DMG_PATH")" = "$DMG_SHA" ] \
&& [ "$(release_staging_sha256 "$NOTARIZE_UPDATER_TARBALL")" = "$NOTARIZE_UPDATER_TARBALL_SHA" ] \
&& [ "$(release_staging_sha256 "$NOTARIZE_UPDATER_TARBALL.sig")" = "$NOTARIZE_UPDATER_SIG_SHA" ]; then
    pass "all three members are back to the submitted bytes"
else
    fail "the recovery left at least one member on the newer build's bytes"
fi
rm -rf "$ROOT"

# THE F3 CASE. Half the set is recoverable and half is not. The old per-artifact
# guard restored the DMG and carried on, which IS the bug: the tarball beside it
# then belonged to the newer build, the manifest recorded both, and the release
# shipped two builds. Refusing is correct; refusing WITHOUT having restored
# anything is what makes it honest, because a half-restored tree invites a
# re-run that then looks consistent.
echo ""
echo "test: a set that is only HALF recoverable refuses and restores nothing"
use_tree "$(new_tree)" with-pairing
notarize_pin_submitted_set "$DMG_PATH" "$DMG_SHA" >/dev/null
# Drop the tarball's pin, so only the DMG can be recovered.
rm -rf "$(notarize_pin_dir "$NOTARIZE_UPDATER_TARBALL_SHA")"
printf 'newer dmg\n'     > "$DMG_PATH"
printf 'newer tarball\n' > "$NOTARIZE_UPDATER_TARBALL"
NEWER_DMG_SHA="$(release_staging_sha256 "$DMG_PATH")"
OUT="$(assert_submitted_set_is_intact "$DMG_SHA" 2>&1)"; RC=$?
if [ $RC -ne 0 ]; then
    pass "a half-recoverable set refuses"
else
    fail "HALF A BUILD WAS ACCEPTED: the DMG was restored next to a newer tarball"
fi
case "$OUT" in
    *"the updater payload"*) pass "the refusal names the member that could not be recovered" ;;
    *) fail "the refusal does not name the unrecoverable member: $OUT" ;;
esac
case "$OUT" in
    *"NOTHING has been restored"*) pass "the refusal states that nothing was restored" ;;
    *) fail "the refusal does not say the tree was left alone: $OUT" ;;
esac
if [ "$(release_staging_sha256 "$DMG_PATH")" = "$NEWER_DMG_SHA" ]; then
    pass "the recoverable member was NOT restored (decide first, then act)"
else
    fail "the DMG was restored anyway, leaving exactly the half-a-build state"
fi
rm -rf "$ROOT"

echo ""
echo "test: a replaced updater payload with NO pin refuses even when the DMG is fine"
use_tree "$(new_tree)" with-pairing
printf 'a LATER build overwrote this\n' > "$NOTARIZE_UPDATER_TARBALL"
OUT="$(assert_submitted_set_is_intact "$DMG_SHA" 2>&1)"; RC=$?
if [ $RC -ne 0 ] && echo "$OUT" | grep -q "the updater payload"; then
    pass "an unpinned, replaced payload refuses on its own"
else
    fail "a replaced updater payload was accepted alongside an intact DMG (rc=$RC): $OUT"
fi
rm -rf "$ROOT"

echo ""
echo "test: an empty pairing asserts the DMG alone"
# A build with no updater key produces no payload at all. The pairing values are
# empty, the two updater triples are skipped, and the DMG is still asserted.
use_tree "$(new_tree)"
printf 'a LATER build overwrote this\n' > "$ROOT/target/release/bundle/macos/Lucidos.app.tar.gz"
if assert_submitted_set_is_intact "$DMG_SHA" 2>/dev/null; then
    pass "no pairing recorded ⇒ the updater artifacts are not asserted"
else
    fail "an empty pairing was treated as a mismatch"
fi
rm -rf "$ROOT"

# ── 9. THE STAPLE IS AN INTENDED MUTATION, NOT A REBUILD (v0.19.1) ───────────
# The real v0.19.1 Phase A sequence, run through the real chokepoint: pin the
# submitted bytes, staple (which rewrites the DMG), then run the assertion
# stage_release_artifacts runs. Before the fix the last step restored the
# pre-staple pin over the ticket.

# Everything up to and including the staple. staple_notarized_artifacts is the
# one function both stapling paths funnel through (the fresh build, and the
# resume behind --attach-notarized), so testing it covers both.
# shellcheck disable=SC2034 # APP_PATH is read by an extracted function
submit_and_staple() {
    SUBMITTED_SHA="$DMG_SHA"
    NOTARIZE_EXPECTED_SHA="$SUBMITTED_SHA"
    APP_PATH=""   # the standalone .app is a separate concern; keep this on the DMG
    NOTARIZE_STATE_FILE=""   # no resume handle in these fixtures; the rewrite no-ops
    notarize_pin_submitted_set "$DMG_PATH" "$SUBMITTED_SHA" >/dev/null
    staple_notarized_artifacts "$NOTARIZE_EXPECTED_SHA" >/dev/null 2>&1
    STAPLED_SHA="$(release_staging_sha256 "$DMG_PATH")"
}

echo ""
echo "test: the staple survives staging, and the manifest records the STAPLED sha"
use_tree "$(new_tree)" with-pairing
submit_and_staple
# Assert on the SIDE EFFECT, not on the return code. staple_notarized_artifacts
# ends in an if/else whose last statement is an echo, so under this suite's
# non-exiting `die` stub it returns 0 whether or not the guards refused: an
# rc check here would be a test that cannot fail.
if [ "$NOTARIZE_EXPECTED_SHA" = "$STAPLED_SHA" ] && [ "$NOTARIZE_EXPECTED_SHA" != "$SUBMITTED_SHA" ]; then
    pass "the expected bytes moved forward to the stapled DMG"
else
    fail "the expected bytes did not move (expected=$NOTARIZE_EXPECTED_SHA stapled=$STAPLED_SHA submitted=$SUBMITTED_SHA)"
fi
if [ "$STAPLED_SHA" != "$SUBMITTED_SHA" ] && has_ticket "$DMG_PATH"; then
    pass "the staple changed the DMG's bytes (the precondition for the bug)"
else
    fail "the fixture did not mutate the DMG, so this proves nothing"
fi

# THE LINE THAT UNDID IT. stage_release_artifacts asserts the set a second time,
# after the staple, before it copies anything into the staging dir.
if assert_submitted_set_is_intact "$NOTARIZE_EXPECTED_SHA" >/dev/null 2>&1; then
    pass "the staging assertion accepts the DMG this run just stapled"
else
    fail "the staging assertion refused the DMG this run just stapled"
fi
if has_ticket "$DMG_PATH" && [ "$(release_staging_sha256 "$DMG_PATH")" = "$STAPLED_SHA" ]; then
    pass "the DMG still carries its ticket after the staging assertion"
else
    fail "THE STAPLE WAS UNDONE: the pre-staple pin was restored over the ticket"
fi

# And the half that reaches a user: what the manifest records is what
# --publish-verified re-verifies and ships.
STAGING="$ROOT/staging"
mkdir -p "$STAGING"
cp "$DMG_PATH" "$NOTARIZE_UPDATER_TARBALL" "$NOTARIZE_UPDATER_TARBALL.sig" "$STAGING/"
RELEASE_STAGING_PLATFORM_KEY="darwin-aarch64" \
RELEASE_STAGING_NOTARIZED="true" \
release_staging_write_manifest "$STAGING" "$VERSION" "0000000000000000000000000000000000000000" \
    "$(basename "$DMG_PATH")" "Lucidos.app.tar.gz" "Lucidos.app.tar.gz.sig" >/dev/null \
    || fail "could not write the staging manifest"
if has_ticket "$STAGING/$(basename "$DMG_PATH")"; then
    pass "the STAGED DMG carries the ticket"
else
    fail "the staged DMG has no ticket: stapler validate would exit 65 on it"
fi
if [ "$(manifest_sha "$STAGING" "$(basename "$DMG_PATH")")" = "$STAPLED_SHA" ]; then
    pass "the manifest records the stapled sha"
else
    fail "the manifest records the pre-staple sha, so --publish-verified would ship an unstapled DMG"
fi
if release_staging_verify "$STAGING" >/dev/null 2>&1; then
    pass "the staging re-verifies against its own manifest"
else
    fail "release_staging_verify rejected the staged set"
fi
rm -rf "$ROOT"

# THE THIRD PLACE THE EXPECTATION LIVES. The resume gate re-hashes the recorded
# artifact and refuses on any mismatch, so a handle left describing the
# pre-staple bytes makes a just-stapled release unresumable. On a deferred
# release that means an already-published DMG no --attach-notarized could ever
# staple, which is the one outcome the handle exists to prevent.
echo ""
echo "test: the resume handle moves with the staple, so a stapled release stays resumable"
use_tree "$(new_tree)" with-pairing
NOTARIZE_STATE_FILE="$ROOT/notarize-state.json"
NOTARIZE_SUBMISSION_ID="00000000-0000-4000-8000-000000000000"
HANDLE_COMMIT="0000000000000000000000000000000000000000"
# shellcheck disable=SC2034 # read by an extracted function
APP_PATH=""
RELEASE_NOTARIZE_UPDATER_TARBALL="$NOTARIZE_UPDATER_TARBALL" \
RELEASE_NOTARIZE_UPDATER_TARBALL_SHA256="$NOTARIZE_UPDATER_TARBALL_SHA" \
RELEASE_NOTARIZE_UPDATER_SIG_SHA256="$NOTARIZE_UPDATER_SIG_SHA" \
release_notarize_write_state "$NOTARIZE_STATE_FILE" "$RELEASE_NOTARIZE_STAGE_DMG" \
    "$NOTARIZE_SUBMISSION_ID" "$DMG_PATH" "$DMG_SHA" "$VERSION" \
    "$HANDLE_COMMIT" "2026-08-02T00:00:00Z" \
    || fail "could not write the fixture resume handle"
notarize_pin_submitted_set "$DMG_PATH" "$DMG_SHA" >/dev/null
NOTARIZE_EXPECTED_SHA="$DMG_SHA"
staple_notarized_artifacts "$NOTARIZE_EXPECTED_SHA" >/dev/null 2>&1
STAPLED_SHA="$(release_staging_sha256 "$DMG_PATH")"
if [ "$(release_notarize_field "$NOTARIZE_STATE_FILE" artifact_sha256)" = "$STAPLED_SHA" ]; then
    pass "the handle records the stapled sha"
else
    fail "the handle still records the pre-staple sha"
fi
if release_notarize_resumable "$NOTARIZE_STATE_FILE" "$HANDLE_COMMIT" >/dev/null 2>&1; then
    pass "the handle is still resumable once the DMG is stapled"
else
    fail "stapling made the release unresumable, stranding a published DMG"
fi
if [ "$(release_notarize_field "$NOTARIZE_STATE_FILE" updater_tarball_sha256)" = "$NOTARIZE_UPDATER_TARBALL_SHA" ] \
&& [ "$(release_notarize_field "$NOTARIZE_STATE_FILE" submission_id)" = "$NOTARIZE_SUBMISSION_ID" ] \
&& [ "$(release_notarize_field "$NOTARIZE_STATE_FILE" source_commit)" = "$HANDLE_COMMIT" ]; then
    pass "the rewrite preserves the pairing, the submission id and the source commit"
else
    fail "the rewrite dropped a field the resume gate needs"
fi
rm -rf "$ROOT"
NOTARIZE_STATE_FILE=""

# The guard must keep BOTH halves past the staple: still detect a rebuild, and
# still recover it, by the STAPLED bytes rather than the submitted ones.
echo ""
echo "test: a rebuild AFTER the staple recovers the STAPLED bytes, not the pin from before it"
use_tree "$(new_tree)" with-pairing
submit_and_staple
printf 'DIFFERENT bytes from a later rebuild\n' > "$DMG_PATH"
if assert_submitted_set_is_intact "$NOTARIZE_EXPECTED_SHA" >/dev/null 2>&1; then
    pass "a post-staple rebuild is still recovered"
else
    fail "a post-staple rebuild could not be recovered, though the stapled bytes were pinned"
fi
if [ "$(release_staging_sha256 "$DMG_PATH")" = "$STAPLED_SHA" ] && has_ticket "$DMG_PATH"; then
    pass "the recovery restored the stapled DMG"
else
    fail "the recovery restored the pre-staple bytes, losing the ticket"
fi
rm -rf "$ROOT"

# The adopt itself has to be defended, because whatever it records is what the
# release stages. A rebuild landing between the staple returning and the hash
# being taken would otherwise be blessed as "the stapled bytes" and pinned as the
# recovery copy, publishing a DMG Apple never saw.
echo ""
echo "test: the expected bytes are NOT moved forward onto a DMG with no ticket"
use_tree "$(new_tree)" with-pairing
NOTARIZE_EXPECTED_SHA="$DMG_SHA"
notarize_pin_submitted_set "$DMG_PATH" "$DMG_SHA" >/dev/null
# What a concurrent rebuild leaves at the fixed path: different bytes, no ticket.
printf 'a rebuild landed in the adopt window\n' > "$DMG_PATH"
notarize_record_stapled_dmg >/dev/null 2>&1; RC=$?
if [ $RC -ne 0 ]; then
    pass "recording refuses when the bytes at the path carry no ticket"
else
    fail "an unstapled, never-submitted DMG was adopted as the stapled bytes"
fi
if [ "$NOTARIZE_EXPECTED_SHA" = "$DMG_SHA" ]; then
    pass "the expected bytes were left where they were"
else
    fail "the expected bytes were moved onto the rebuild anyway"
fi
if [ -z "$(notarize_find_pin "$(release_staging_sha256 "$DMG_PATH")")" ]; then
    pass "the rebuild was not pinned as a recovery copy"
else
    fail "the rebuild was pinned, so a later recovery would restore it"
fi
rm -rf "$ROOT"

# "no ticket" and "could not ask" demand opposite recoveries: a rebuild plus a
# fresh notarization, versus `xcode-select --install`. Reporting the second as
# the first is a 40-minute answer to a one-minute problem.
echo ""
echo "test: a broken stapler is reported as a toolchain failure, not as a missing ticket"
use_tree "$(new_tree)" with-pairing
printf 'BROKEN-TOOLCHAIN\n' >> "$DMG_PATH"
OUT="$(dmg_ticket_is_stapled "$DMG_PATH" 2>&1)"; RC=$?
if [ $RC -ne 0 ]; then
    pass "an unusable stapler refuses rather than reporting a verdict"
else
    fail "a stapler that could not answer was read as 'stapled'"
fi
case "$OUT" in
    *"toolchain failure"*|*"xcode-select"*) pass "the refusal names the toolchain, not the DMG" ;;
    *) fail "the refusal blames the DMG for a tooling problem: '$OUT'" ;;
esac
case "$OUT" in
    *"requires Xcode"*) pass "stapler's own words are quoted rather than swallowed" ;;
    *) fail "stapler's stderr was discarded: '$OUT'" ;;
esac
rm -rf "$ROOT"

echo ""
echo "test: a post-staple rebuild with no stapled pin REFUSES (it does not fall back to the unstapled pin)"
use_tree "$(new_tree)" with-pairing
submit_and_staple
# Only the stapled copy is dropped. The PRE-staple pin is deliberately left in
# place: falling back to it is exactly the v0.19.1 failure, so an unrecoverable
# set must refuse rather than restore a DMG with no ticket.
rm -rf "$(notarize_pin_dir "$STAPLED_SHA")"
printf 'DIFFERENT bytes from a later rebuild\n' > "$DMG_PATH"
REBUILT_SHA="$(release_staging_sha256 "$DMG_PATH")"
OUT="$(assert_submitted_set_is_intact "$NOTARIZE_EXPECTED_SHA" 2>&1)"; RC=$?
if [ $RC -ne 0 ]; then
    pass "an unrecoverable post-staple rebuild refuses (rc=$RC)"
else
    fail "a rebuild after the staple was accepted"
fi
case "$OUT" in
    *"REFUSING TO STAPLE OR STAGE"*) pass "the refusal is explicit about why" ;;
    *)                               fail "unclear refusal: '$OUT'" ;;
esac
if [ "$(release_staging_sha256 "$DMG_PATH")" = "$REBUILT_SHA" ]; then
    pass "nothing was restored, so the unstapled pin was not silently substituted"
else
    fail "the pre-staple pin was restored over the rebuild, which is the bug in another shape"
fi
rm -rf "$ROOT"

# ── 10. the triple-argument contract ─────────────────────────────────────────
echo ""
echo "test: a recorded checksum with no path REFUSES rather than skipping"
# The hole this closes: skipping a triple whose path is empty drops a member from
# the set silently, which is the whole class the set assertion exists to prevent.
# Only an empty CHECKSUM means "nothing recorded, nothing to assert".
use_tree "$(new_tree)"
OUT="$(assert_submitted_artifacts_are_intact "the updater payload" "" "$DMG_SHA" 2>&1)"; RC=$?
if [ $RC -ne 0 ] && echo "$OUT" | grep -q "no path was given"; then
    pass "a checksum with no path refuses"
else
    fail "an unpathed member was skipped instead of refused (rc=$RC): $OUT"
fi
if assert_submitted_artifacts_are_intact "the updater payload" "" "" 2>/dev/null; then
    pass "an empty checksum with no path is still a no-op"
else
    fail "a wholly empty member was treated as a failure"
fi
rm -rf "$ROOT"

echo ""
echo "test: a trailing argument is refused rather than silently dropped"
use_tree "$(new_tree)"
if assert_submitted_artifacts_are_intact "label" "$DMG_PATH" 2>/dev/null; then
    fail "an incomplete triple was accepted, so a member would go unchecked"
else
    pass "an incomplete triple refuses"
fi
rm -rf "$ROOT"

rm -rf "$FAKE_BIN"

echo ""
echo "release_staple_guard: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
