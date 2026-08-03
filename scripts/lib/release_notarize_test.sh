#!/usr/bin/env bash
# Tests for scripts/lib/release_notarize.sh — the resume handle that makes the
# notarization stage survive losing the process waiting on Apple. Pure functions
# (python3 json/hashlib, no xcrun, no network), so the whole matrix runs offline
# against fake DMG files. Run: ./scripts/lib/release_notarize_test.sh
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# release_notarize.sh uses release_staging_sha256, so the staging lib comes first
# (the same ordering build-dmg.sh and headless_tarball.sh rely on).
# shellcheck source=scripts/lib/release_staging.sh
source "$SCRIPT_DIR/release_staging.sh"
# shellcheck source=scripts/lib/release_notarize.sh
source "$SCRIPT_DIR/release_notarize.sh"

PASS=0
FAIL=0
fail() { echo "  FAIL: $*"; FAIL=$((FAIL+1)); }
pass() { echo "  ok:   $*"; PASS=$((PASS+1)); }

SUBMISSION="ca4778bf-bf0e-4c04-9f2c-7459885cdb51"
VERSION="0.77.0"   # synthetic: a fixture must never collide with a real release version
COMMIT="9e8f186e2aa0c0ffee1234567890abcdef123456"
SUBMITTED="2026-07-28T08:22:00Z"

# A fixture tree: a fake signed DMG, the updater trio it was PAIRED with at
# submit time, and a stage `dmg` state file recording all of it. The pairing is
# what F3 added: without it, staging pairs the recorded DMG with whatever
# .app.tar.gz happens to be on disk, which a concurrent rebuild can replace.
new_fixture() {
    local root dmg tarball
    root="$(mktemp -d)"
    mkdir -p "$root/target/release/bundle/dmg" "$root/target/release/bundle/macos"
    dmg="$root/target/release/bundle/dmg/Lucidos_${VERSION}_aarch64.dmg"
    tarball="$root/target/release/bundle/macos/Lucidos.app.tar.gz"
    printf 'fake signed dmg payload\n' > "$dmg"
    printf 'fake updater payload\n'    > "$tarball"
    printf 'fake updater signature\n'  > "$tarball.sig"
    RELEASE_NOTARIZE_UPDATER_TARBALL="$tarball" \
    RELEASE_NOTARIZE_UPDATER_TARBALL_SHA256="$(release_staging_sha256 "$tarball")" \
    RELEASE_NOTARIZE_UPDATER_SIG_SHA256="$(release_staging_sha256 "$tarball.sig")" \
    release_notarize_write_state \
        "$(release_notarize_state_path "$root" "$VERSION")" \
        "$RELEASE_NOTARIZE_STAGE_DMG" "$SUBMISSION" "$dmg" \
        "$(release_staging_sha256 "$dmg")" "$VERSION" "$COMMIT" "$SUBMITTED" \
        || return 1
    printf '%s' "$root"
}

# The same tree, but with the state file rewritten to the PRE-2026-08-02 shape:
# no stage, no pairing, and the old dmg_path / dmg_sha256 names.
downgrade_fixture_to_legacy_handle() {  # <root>
    local root="$1" state dmg
    state="$(release_notarize_state_path "$root" "$VERSION")"
    dmg="$root/target/release/bundle/dmg/Lucidos_${VERSION}_aarch64.dmg"
    printf '{\n  "submission_id": "%s",\n  "dmg_path": "%s",\n  "version": "%s",\n  "dmg_sha256": "%s",\n  "source_commit": "%s",\n  "submitted_at": "%s"\n}\n' \
        "$SUBMISSION" "$dmg" "$VERSION" "$(release_staging_sha256 "$dmg")" \
        "$COMMIT" "$SUBMITTED" > "$state"
}

# ── state_path ────────────────────────────────────────────────────────────────
echo "test: state_path is <repo-root>/.lucidos/release-state/notarize-<version>.json"
got="$(release_notarize_state_path /tmp/repo 1.2.3)"
if [ "$got" = "/tmp/repo/.lucidos/release-state/notarize-1.2.3.json" ]; then
    pass "state_path composes the expected path"
else
    fail "state_path returned '$got'"
fi

# ── write / read round-trip ───────────────────────────────────────────────────
echo ""
echo "test: write_state round-trips every recorded field"
ROOT="$(new_fixture)"
STATE="$(release_notarize_state_path "$ROOT" "$VERSION")"
DMG="$ROOT/target/release/bundle/dmg/Lucidos_${VERSION}_aarch64.dmg"
if [ -f "$STATE" ]; then
    pass "write_state created the state file (and its parent dir)"
else
    fail "write_state did not create $STATE"
fi
check_field() {  # <field> <expected>
    local got
    got="$(release_notarize_field "$STATE" "$1" 2>/dev/null)"
    if [ "$got" = "$2" ]; then pass "$1 = $2"; else fail "$1 was '$got', expected '$2'"; fi
}
TARBALL="$ROOT/target/release/bundle/macos/Lucidos.app.tar.gz"
check_field stage                  "$RELEASE_NOTARIZE_STAGE_DMG"
check_field submission_id          "$SUBMISSION"
check_field artifact_path          "$DMG"
check_field version                "$VERSION"
check_field artifact_sha256        "$(release_staging_sha256 "$DMG")"
check_field source_commit          "$COMMIT"
check_field submitted_at           "$SUBMITTED"
check_field updater_tarball_path   "$TARBALL"
check_field updater_tarball_sha256 "$(release_staging_sha256 "$TARBALL")"
check_field updater_sig_sha256     "$(release_staging_sha256 "$TARBALL.sig")"
# Every key is written even when it does not apply to this stage, so a reader can
# tell "empty because stage dmg has no app" from "absent because the handle
# predates the field". That distinction is what INV-3 rests on.
check_field app_path   ""
check_field app_cdhash ""

out="$(release_notarize_field "$STATE" no_such_field 2>&1)"; rc=$?
if [ $rc -ne 0 ] && echo "$out" | grep -q "no 'no_such_field' field"; then
    pass "field reports an absent field"
else
    fail "field accepted an absent field (rc=$rc): $out"
fi
rm -rf "$ROOT"

# ── the resume gate: happy path ───────────────────────────────────────────────
echo ""
echo "test: resumable accepts an intact handle"
ROOT="$(new_fixture)"
STATE="$(release_notarize_state_path "$ROOT" "$VERSION")"
out="$(release_notarize_resumable "$STATE" "$COMMIT" 2>&1)"; rc=$?
if [ $rc -eq 0 ]; then
    pass "resumable exits 0 for an untouched DMG on the recorded commit"
else
    fail "resumable rejected an intact handle (rc=$rc): $out"
fi
rm -rf "$ROOT"

# ── the resume gate: a mismatched DMG checksum refuses to resume ──────────────
echo ""
echo "test: resumable refuses a DMG whose checksum no longer matches"
ROOT="$(new_fixture)"
STATE="$(release_notarize_state_path "$ROOT" "$VERSION")"
printf 'tampered\n' >> "$ROOT/target/release/bundle/dmg/Lucidos_${VERSION}_aarch64.dmg"
out="$(release_notarize_resumable "$STATE" "$COMMIT" 2>&1)"; rc=$?
if [ $rc -ne 0 ] && echo "$out" | grep -qi "checksum mismatch"; then
    pass "a rebuilt/tampered DMG is refused — Apple scanned different bytes"
else
    fail "resumable accepted a checksum-mismatched DMG (rc=$rc): $out"
fi
rm -rf "$ROOT"

# ── the resume gate: the DMG is gone ──────────────────────────────────────────
echo ""
echo "test: resumable refuses when the submitted DMG is gone"
ROOT="$(new_fixture)"
STATE="$(release_notarize_state_path "$ROOT" "$VERSION")"
rm -f "$ROOT/target/release/bundle/dmg/Lucidos_${VERSION}_aarch64.dmg"
out="$(release_notarize_resumable "$STATE" "$COMMIT" 2>&1)"; rc=$?
if [ $rc -ne 0 ] && echo "$out" | grep -qi "is gone"; then
    pass "a missing DMG is refused"
else
    fail "resumable accepted a missing DMG (rc=$rc): $out"
fi
rm -rf "$ROOT"

# ── the resume gate: the tree moved ───────────────────────────────────────────
# Load-bearing: the RESUMING run stamps staging manifest.json with its own HEAD,
# so resuming on a moved tree would claim a commit the DMG was never built from —
# and release.sh --publish-verified's identity guard would then pass on a lie.
echo ""
echo "test: resumable refuses when the source commit moved"
ROOT="$(new_fixture)"
STATE="$(release_notarize_state_path "$ROOT" "$VERSION")"
out="$(release_notarize_resumable "$STATE" "0000000000000000000000000000000000000000" 2>&1)"; rc=$?
if [ $rc -ne 0 ] && echo "$out" | grep -qi "source-commit mismatch"; then
    pass "a moved tree is refused"
else
    fail "resumable accepted a moved source commit (rc=$rc): $out"
fi
rm -rf "$ROOT"

# ── the resume gate: missing / empty inputs ───────────────────────────────────
echo ""
echo "test: resumable refuses a missing state file and an empty expected commit"
out="$(release_notarize_resumable "/nonexistent/notarize-9.9.9.json" "$COMMIT" 2>&1)"; rc=$?
if [ $rc -ne 0 ] && echo "$out" | grep -qi "no notarize state"; then
    pass "a missing state file is refused"
else
    fail "resumable accepted a missing state file (rc=$rc): $out"
fi

ROOT="$(new_fixture)"
STATE="$(release_notarize_state_path "$ROOT" "$VERSION")"
out="$(release_notarize_resumable "$STATE" "" 2>&1)"; rc=$?
if [ $rc -ne 0 ] && echo "$out" | grep -qi "expected source commit"; then
    pass "an empty expected commit is refused (never resume unverified)"
else
    fail "resumable accepted an empty expected commit (rc=$rc): $out"
fi
rm -rf "$ROOT"

echo ""
echo "test: write_state refuses an empty submission id and an unknown stage"
out="$(release_notarize_write_state "$(mktemp -d)/state.json" dmg "" /tmp/x.dmg sha 1.2.3 commit ts 2>&1)"; rc=$?
if [ $rc -ne 0 ] && echo "$out" | grep -qi "submission id"; then
    pass "write_state refuses an empty submission id"
else
    fail "write_state accepted an empty submission id (rc=$rc): $out"
fi
out="$(release_notarize_write_state "$(mktemp -d)/state.json" bogus "$SUBMISSION" /tmp/x.dmg sha 1.2.3 commit ts 2>&1)"; rc=$?
if [ $rc -ne 0 ] && echo "$out" | grep -qi "stage"; then
    pass "write_state refuses a stage that is neither app nor dmg"
else
    fail "write_state accepted an unknown stage (rc=$rc): $out"
fi

# ── the two stages ────────────────────────────────────────────────────────────
echo ""
echo "test: a stage 'app' handle round-trips the app identity"
# The app zip is what is submitted; the app's CDHash is what the ticket is issued
# for, and therefore what must be re-asserted before stapling. A file hash of the
# zip could not serve: `ditto -c -k` is not byte-reproducible, so re-zipping and
# comparing would report false mismatches.
ROOT="$(mktemp -d)"
APP_ZIP="$ROOT/Lucidos.app.notarize.zip"
printf 'pretend app zip\n' > "$APP_ZIP"
APP_STATE="$ROOT/notarize-app.json"
RELEASE_NOTARIZE_APP_PATH="$ROOT/Lucidos.app" \
RELEASE_NOTARIZE_APP_CDHASH="d3974ae45fa91b7a9df11b9b5e52eb988532a7cb" \
release_notarize_write_state "$APP_STATE" "$RELEASE_NOTARIZE_STAGE_APP" "$SUBMISSION" \
    "$APP_ZIP" "$(release_staging_sha256 "$APP_ZIP")" "$VERSION" "$COMMIT" "$SUBMITTED"
if [ "$(release_notarize_field "$APP_STATE" stage)" = "app" ] \
   && [ "$(release_notarize_field "$APP_STATE" artifact_path)" = "$APP_ZIP" ] \
   && [ "$(release_notarize_field "$APP_STATE" app_cdhash)" = "d3974ae45fa91b7a9df11b9b5e52eb988532a7cb" ]; then
    pass "the app stage records the zip it submitted and the cdhash it must staple"
else
    fail "the app-stage handle did not round-trip"
fi
rm -rf "$ROOT"

echo ""
echo "test: resumable refuses a handle recording an unknown stage"
ROOT="$(new_fixture)"
STATE="$(release_notarize_state_path "$ROOT" "$VERSION")"
python3 - "$STATE" <<'PY'
import json, sys
with open(sys.argv[1]) as f:
    state = json.load(f)
state["stage"] = "halfway"
with open(sys.argv[1], "w") as f:
    json.dump(state, f)
PY
out="$(release_notarize_resumable "$STATE" "$COMMIT" 2>&1)"; rc=$?
if [ $rc -ne 0 ] && echo "$out" | grep -qi "unknown stage"; then
    pass "an unrecognised stage is refused"
else
    fail "resumable accepted an unknown stage (rc=$rc): $out"
fi
rm -rf "$ROOT"

# ── the paired set (F3) ───────────────────────────────────────────────────────
# The finding: the handle pinned the DMG and nothing tied the .app.tar.gz + .sig
# to the same build, so a concurrent rebuild during the notary wait left the
# recovery branch restoring build N's DMG next to build N+1's tarball, and the
# release shipped two different builds.
echo ""
echo "test: resumable refuses when the paired updater payload was replaced"
ROOT="$(new_fixture)"
STATE="$(release_notarize_state_path "$ROOT" "$VERSION")"
printf 'a LATER build overwrote this\n' > "$ROOT/target/release/bundle/macos/Lucidos.app.tar.gz"
out="$(release_notarize_resumable "$STATE" "$COMMIT" 2>&1)"; rc=$?
if [ $rc -ne 0 ] && echo "$out" | grep -qi "updater-payload mismatch"; then
    pass "a replaced updater payload refuses"
else
    fail "resumable accepted a replaced updater payload (rc=$rc): $out"
fi
if echo "$out" | grep -qi "two different builds"; then
    pass "the refusal says what shipping it would mean"
else
    fail "the refusal does not explain the consequence: $out"
fi
rm -rf "$ROOT"

echo ""
echo "test: resumable refuses when the paired updater SIGNATURE was replaced"
# Its own case: a .sig from a different build makes every updater reject the
# update, which is a louder failure than the payload mismatch and a different one.
ROOT="$(new_fixture)"
STATE="$(release_notarize_state_path "$ROOT" "$VERSION")"
printf 'a signature over other bytes\n' > "$ROOT/target/release/bundle/macos/Lucidos.app.tar.gz.sig"
out="$(release_notarize_resumable "$STATE" "$COMMIT" 2>&1)"; rc=$?
if [ $rc -ne 0 ] && echo "$out" | grep -qi "updater-signature mismatch"; then
    pass "a replaced updater signature refuses"
else
    fail "resumable accepted a replaced updater signature (rc=$rc): $out"
fi
rm -rf "$ROOT"

echo ""
echo "test: resumable refuses when the paired updater payload is gone"
ROOT="$(new_fixture)"
STATE="$(release_notarize_state_path "$ROOT" "$VERSION")"
rm -f "$ROOT/target/release/bundle/macos/Lucidos.app.tar.gz"
out="$(release_notarize_resumable "$STATE" "$COMMIT" 2>&1)"; rc=$?
if [ $rc -ne 0 ] && echo "$out" | grep -qi "is gone"; then
    pass "a deleted updater payload refuses"
else
    fail "resumable accepted a missing updater payload (rc=$rc): $out"
fi
rm -rf "$ROOT"

echo ""
echo "test: an EMPTY pairing is vacuous, not a refusal"
# A local signed build with no updater key produces no .app.tar.gz at all. That
# writes the keys empty, and the pairing check has nothing to compare. It must
# not refuse: the release-grade refusal for a missing payload belongs to
# stage_release_artifacts, which says so in terms the operator can act on.
ROOT="$(mktemp -d)"
mkdir -p "$ROOT/target/release/bundle/dmg"
DMG="$ROOT/target/release/bundle/dmg/Lucidos_${VERSION}_aarch64.dmg"
printf 'fake signed dmg payload\n' > "$DMG"
STATE="$(release_notarize_state_path "$ROOT" "$VERSION")"
release_notarize_write_state "$STATE" "$RELEASE_NOTARIZE_STAGE_DMG" "$SUBMISSION" \
    "$DMG" "$(release_staging_sha256 "$DMG")" "$VERSION" "$COMMIT" "$SUBMITTED"
out="$(release_notarize_resumable "$STATE" "$COMMIT" 2>&1)"; rc=$?
if [ $rc -eq 0 ]; then
    pass "a build that produced no updater payload still resumes"
else
    fail "an empty pairing was treated as a mismatch (rc=$rc): $out"
fi
rm -rf "$ROOT"

echo ""
echo "test: a handle written BEFORE the paired shape is refused by name"
# Explicitly, rather than read as "no pairing recorded, so nothing can mismatch".
# Absent is not the same as empty, and only one of them may pass.
ROOT="$(new_fixture)"
STATE="$(release_notarize_state_path "$ROOT" "$VERSION")"
downgrade_fixture_to_legacy_handle "$ROOT"
out="$(release_notarize_resumable "$STATE" "$COMMIT" 2>&1)"; rc=$?
if [ $rc -ne 0 ] && echo "$out" | grep -qi "predates the paired-set notarize handle"; then
    pass "a pre-2026-08-02 handle is refused by name"
else
    fail "a legacy handle was accepted or refused unclearly (rc=$rc): $out"
fi
if echo "$out" | grep -q "rm $STATE"; then
    pass "the refusal prints the exact recovery command"
else
    fail "the refusal does not say how to recover: $out"
fi
# has_fields is the primitive that draws the line; assert it directly too.
# shellcheck disable=SC2086 # the field list is deliberately word-split
if release_notarize_has_fields "$STATE" $RELEASE_NOTARIZE_REQUIRED_FIELDS; then
    fail "has_fields reported the legacy handle as complete"
else
    pass "has_fields tells an absent key from an empty one"
fi
rm -rf "$ROOT"

# ── clear ─────────────────────────────────────────────────────────────────────
echo ""
echo "test: clear drops a spent handle and is a no-op otherwise"
ROOT="$(new_fixture)"
STATE="$(release_notarize_state_path "$ROOT" "$VERSION")"
release_notarize_clear "$STATE"
if [ ! -f "$STATE" ]; then pass "clear removed the state file"; else fail "clear left $STATE behind"; fi
if release_notarize_clear "$STATE" && release_notarize_clear ""; then
    pass "clear is a no-op for an already-gone file and an empty path"
else
    fail "clear returned non-zero for a no-op case"
fi
rm -rf "$ROOT"

# ── submission-id shape ───────────────────────────────────────────────────────
echo ""
echo "test: valid_submission_id accepts a notary UUID, rejects anything else"
if release_notarize_valid_submission_id "$SUBMISSION"; then
    pass "accepts a real submission UUID"
else
    fail "rejected a real submission UUID"
fi
for bad in "" "not-a-uuid" "ca4778bf" "ca4778bf-bf0e-4c04-9f2c-7459885cdb5" \
           "ca4778bf-bf0e-4c04-9f2c-7459885cdb51x" "ca4778bg-bf0e-4c04-9f2c-7459885cdb51"; do
    if release_notarize_valid_submission_id "$bad"; then
        fail "accepted a malformed submission id: '$bad'"
    else
        pass "rejects '$bad'"
    fi
done

# ── notarytool JSON parsing ───────────────────────────────────────────────────
echo ""
echo "test: json_field reads notarytool's --output-format json replies"
got="$(printf '{"id":"%s","message":"Successfully uploaded file"}' "$SUBMISSION" \
        | release_notarize_json_field id)"
if [ "$got" = "$SUBMISSION" ]; then
    pass "extracts the submission id from a submit reply"
else
    fail "submit-reply id was '$got'"
fi

got="$(printf '{"id":"%s","status":"In Progress"}' "$SUBMISSION" \
        | release_notarize_json_field status)"
if [ "$got" = "In Progress" ]; then
    pass "extracts an 'In Progress' status"
else
    fail "info-reply status was '$got'"
fi

got="$(printf '{"id":"%s","status":"Accepted"}' "$SUBMISSION" \
        | release_notarize_json_field status)"
if [ "$got" = "Accepted" ]; then
    pass "extracts an 'Accepted' status"
else
    fail "status was '$got'"
fi

got="$(printf '{"id":"x"}' | release_notarize_json_field status)"
if [ -z "$got" ]; then
    pass "an absent field yields empty output"
else
    fail "absent field gave '$got'"
fi

out="$(printf 'Error: HTTP status code: 404\n' | release_notarize_json_field status 2>&1)"; rc=$?
if [ $rc -ne 0 ]; then
    pass "non-JSON output (an error message) is a parse failure, not an empty status"
else
    fail "json_field accepted non-JSON input: $out"
fi

echo ""
echo "release_notarize: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
