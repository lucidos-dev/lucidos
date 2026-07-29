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

# A fixture tree: a fake signed DMG plus a state file recording it.
new_fixture() {
    local root dmg
    root="$(mktemp -d)"
    mkdir -p "$root/target/release/bundle/dmg"
    dmg="$root/target/release/bundle/dmg/Lucidos_${VERSION}_aarch64.dmg"
    printf 'fake signed dmg payload\n' > "$dmg"
    release_notarize_write_state \
        "$(release_notarize_state_path "$root" "$VERSION")" \
        "$SUBMISSION" "$dmg" "$VERSION" \
        "$(release_staging_sha256 "$dmg")" "$COMMIT" "$SUBMITTED" \
        || return 1
    printf '%s' "$root"
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
check_field submission_id "$SUBMISSION"
check_field dmg_path      "$DMG"
check_field version       "$VERSION"
check_field dmg_sha256    "$(release_staging_sha256 "$DMG")"
check_field source_commit "$COMMIT"
check_field submitted_at  "$SUBMITTED"

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
echo "test: write_state refuses an empty submission id"
out="$(release_notarize_write_state "$(mktemp -d)/state.json" "" /tmp/x.dmg 1.2.3 sha commit ts 2>&1)"; rc=$?
if [ $rc -ne 0 ] && echo "$out" | grep -qi "submission id"; then
    pass "write_state refuses an empty submission id"
else
    fail "write_state accepted an empty submission id (rc=$rc): $out"
fi

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
