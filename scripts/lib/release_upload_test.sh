#!/usr/bin/env bash
# Tests for scripts/lib/release_upload.sh, the asset-attach ordering that keeps
# latest.json from advertising a payload that is not there yet.
# Run: ./scripts/lib/release_upload_test.sh
#
# THE BUG THIS PINS DOWN (F8 in docs/audits/2026-08-02-macos-update-path-audit.md).
# One `gh release upload` for all four assets uploads them concurrently, so the
# smallest finishes first, and latest.json is the smallest. The updater reads
# `…/releases/latest/download/latest.json`, the release is already marked latest
# when the upload starts, and the manifest names a Lucidos.app.tar.gz on the same
# tag. Measured windows: 10 s on v0.19.0, 65 s on v0.15.0, and 8h06m on v0.16.0.
#
# Two tiers, both offline on any host:
#   • the verdict is a pure function over captured `gh release view --json assets`
#     output, so every accept/refuse case runs with no network;
#   • the ordering is driven end to end through a FAKE `gh` on PATH, which
#     records its invocations. That is what pins "the manifest is not in the
#     first batch" and "a failed presence check uploads no manifest at all",
#     neither of which the pure predicate can express.
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
# shellcheck source=scripts/lib/release_upload.sh
source "$SCRIPT_DIR/release_upload.sh"

PASS=0
FAIL=0
fail() { echo "  FAIL: $*"; FAIL=$((FAIL+1)); }
pass() { echo "  ok:   $*"; PASS=$((PASS+1)); }

WORK="$(mktemp -d)"
cleanup() { rm -rf "$WORK"; }
trap cleanup EXIT

# Never let a real `gh` (or a real network) be reached, and keep the bounded
# retry from making a failing case take 15 s.
RELEASE_UPLOAD_VERIFY_ATTEMPTS=2
RELEASE_UPLOAD_VERIFY_INTERVAL=0

# ── 1. The pure predicate over captured `gh release view --json assets` ──────
# The real shape, with the sizes shrunk. `state` is what GitHub reports while an
# asset is still being processed.
assets_json() {  # <tarball-size> [<tarball-state>]
    cat <<JSON
{"assets":[
  {"name":"Lucidos_0.77.0_aarch64.dmg","size":140,"state":"uploaded","contentType":"application/octet-stream"},
  {"name":"Lucidos.app.tar.gz","size":${1},"state":"${2:-uploaded}","contentType":"application/gzip"},
  {"name":"Lucidos.app.tar.gz.sig","size":12,"state":"uploaded","contentType":"application/octet-stream"}
]}
JSON
}

echo "test: the predicate accepts a release carrying every expected asset"
if assets_json 70 | release_upload_assets_present \
        Lucidos_0.77.0_aarch64.dmg 140 Lucidos.app.tar.gz 70 Lucidos.app.tar.gz.sig 12 2>/dev/null; then
    pass "all present, all the right size"
else
    fail "a complete release listing was refused"
fi

echo ""
echo "test: the predicate refuses an absent asset and names it"
out="$(assets_json 70 | release_upload_assets_present \
        Lucidos_0.77.0_aarch64.dmg 140 latest.json 99 2>&1)"; rc=$?
if [ $rc -ne 0 ] && echo "$out" | grep -q "latest.json is not on the release yet"; then
    pass "an absent asset is refused by name"
else
    fail "expected an absent-asset refusal; got rc=$rc out: $out"
fi

echo ""
echo "test: the predicate refuses a TRUNCATED asset"
# The failure a bare "is the name there" check cannot see, and the one that
# matters: the name appears the moment the upload starts.
out="$(assets_json 3 | release_upload_assets_present Lucidos.app.tar.gz 70 2>&1)"; rc=$?
if [ $rc -ne 0 ] && echo "$out" | grep -q "at 3 bytes, expected 70"; then
    pass "a short asset is refused, with both sizes named"
else
    fail "expected a size refusal; got rc=$rc out: $out"
fi

echo ""
echo "test: the predicate refuses an asset GitHub is still processing"
out="$(assets_json 70 starter | release_upload_assets_present Lucidos.app.tar.gz 70 2>&1)"; rc=$?
if [ $rc -ne 0 ] && echo "$out" | grep -q "state"; then
    pass "state != uploaded is refused"
else
    fail "expected a state refusal; got rc=$rc out: $out"
fi

echo ""
echo "test: a MISSING state field is accepted (it is advisory, size is the check)"
# A future gh that stopped emitting `state` must not break every release.
out="$(printf '%s' '{"assets":[{"name":"Lucidos.app.tar.gz","size":70}]}' \
        | release_upload_assets_present Lucidos.app.tar.gz 70 2>&1)"; rc=$?
if [ $rc -eq 0 ]; then
    pass "an asset with no state field but the right size is accepted"
else
    fail "a missing state field was treated as a failure: $out"
fi

echo ""
echo "test: the predicate fails CLOSED on input it cannot read"
for bad in 'not json at all' '[]' '{}' '{"assets":"nope"}'; do
    out="$(printf '%s' "$bad" | release_upload_assets_present Lucidos.app.tar.gz 70 2>&1)"; rc=$?
    if [ $rc -ne 0 ]; then
        pass "refuses '$bad'"
    else
        fail "accepted unreadable input '$bad' as a pass"
    fi
done
out="$(assets_json 70 | release_upload_assets_present Lucidos.app.tar.gz 2>&1)"; rc=$?
if [ $rc -ne 0 ]; then
    pass "refuses an odd number of name/size arguments"
else
    fail "accepted a name with no size"
fi

echo ""
echo "test: an asset name containing a space is paired with its own size"
# The pairs travel through argv rather than a space-joined string. Joined, a
# spaced name would shift every later name onto the wrong size and report
# nonsense. No shipped artifact has a space today, which is what would have kept
# the trap invisible.
SPACED='{"assets":[{"name":"Lucidos Installer.dmg","size":140,"state":"uploaded"},{"name":"latest.json","size":9,"state":"uploaded"}]}'
if printf '%s' "$SPACED" | release_upload_assets_present "Lucidos Installer.dmg" 140 latest.json 9 2>/dev/null; then
    pass "a spaced asset name is matched against its own size"
else
    fail "a spaced asset name was mis-paired"
fi
out="$(printf '%s' "$SPACED" | release_upload_assets_present "Lucidos Installer.dmg" 1 latest.json 9 2>&1)"; rc=$?
if [ $rc -ne 0 ] && echo "$out" | grep -q "Lucidos Installer.dmg is on the release at 140 bytes, expected 1"; then
    pass "a wrong size for a spaced name is still caught, and named in full"
else
    fail "a spaced name did not report its own size mismatch: $out"
fi

# ── 2. The ordering, through a fake gh ───────────────────────────────────────
# The fake records each invocation and answers `release view` from a JSON file
# the test controls, so "which assets exist" is a property of the fixture rather
# than of a network.

FAKEBIN="$WORK/bin"
mkdir -p "$FAKEBIN"
cat > "$FAKEBIN/gh" <<'FAKE'
#!/usr/bin/env bash
printf '%s\n' "$*" >> "$GH_CALLS"
case "$2" in
    view)   cat "$GH_ASSETS" ;;
    upload) [ -n "${GH_UPLOAD_FAILS:-}" ] && exit 1 ;;
esac
exit 0
FAKE
chmod +x "$FAKEBIN/gh"
export PATH="$FAKEBIN:$PATH"

new_release_fixture() {  # <name>
    local d="$WORK/$1"
    mkdir -p "$d"
    printf 'pretend dmg bytes\n'     > "$d/Lucidos_0.77.0_aarch64.dmg"
    printf 'pretend tarball bytes\n' > "$d/Lucidos.app.tar.gz"
    printf 'sig\n'                   > "$d/Lucidos.app.tar.gz.sig"
    printf '{"version":"0.77.0"}\n'  > "$d/latest.json"
    printf '%s' "$d"
}

# Compose an asset listing that reports each named file at its real local size.
write_assets() {  # <assets-file> <dir> <name>…
    local out="$1" dir="$2"; shift 2
    local name first=1
    { printf '{"assets":['
      for name in "$@"; do
          [ "$first" = "1" ] || printf ','
          first=0
          printf '{"name":"%s","size":%s,"state":"uploaded"}' \
              "$name" "$(release_upload_file_size "$dir/$name")"
      done
      printf ']}'
    } > "$out"
}

echo ""
echo "test: the artifacts go up first, latest.json only after they are verified"
D="$(new_release_fixture happy)"
export GH_CALLS="$WORK/calls-happy" GH_ASSETS="$WORK/assets-happy"
: > "$GH_CALLS"
write_assets "$GH_ASSETS" "$D" Lucidos_0.77.0_aarch64.dmg Lucidos.app.tar.gz Lucidos.app.tar.gz.sig
if release_upload_artifacts_then_manifest v0.77.0 example-org/example-repo "$D/latest.json" \
        "$D/Lucidos_0.77.0_aarch64.dmg" "$D/Lucidos.app.tar.gz" "$D/Lucidos.app.tar.gz.sig" 2>&1; then
    pass "the happy path completes"
else
    fail "the happy path failed"
fi
FIRST_UPLOAD="$(grep -n 'release upload' "$GH_CALLS" | head -1)"
LAST_UPLOAD="$(grep -n 'release upload' "$GH_CALLS" | tail -1)"
VIEW_LINE="$(grep -n 'release view' "$GH_CALLS" | head -1 | cut -d: -f1)"
if [ "$(grep -c 'release upload' "$GH_CALLS")" -eq 2 ]; then
    pass "the upload happens in exactly two calls"
else
    fail "expected two upload calls, got: $(cat "$GH_CALLS")"
fi
case "$FIRST_UPLOAD" in
    *latest.json*) fail "latest.json was in the FIRST upload batch: $FIRST_UPLOAD" ;;
    *Lucidos.app.tar.gz*) pass "the first batch carries the artifacts and NOT the manifest" ;;
    *) fail "unexpected first upload: $FIRST_UPLOAD" ;;
esac
case "$LAST_UPLOAD" in
    *latest.json*) pass "the last call uploads latest.json" ;;
    *) fail "the last upload is not latest.json: $LAST_UPLOAD" ;;
esac
if [ -n "$VIEW_LINE" ] \
   && [ "$VIEW_LINE" -gt "${FIRST_UPLOAD%%:*}" ] && [ "$VIEW_LINE" -lt "${LAST_UPLOAD%%:*}" ]; then
    pass "the presence check runs BETWEEN the two uploads"
else
    fail "the presence check is not between the uploads (first=${FIRST_UPLOAD%%:*} view=$VIEW_LINE last=${LAST_UPLOAD%%:*})"
fi

echo ""
echo "test: a payload that never appears means latest.json is NEVER uploaded"
# The v0.16.0 shape, and the whole point of the change: the manifest must not go
# up while the thing it points at is absent.
D="$(new_release_fixture missing)"
export GH_CALLS="$WORK/calls-missing" GH_ASSETS="$WORK/assets-missing"
: > "$GH_CALLS"
write_assets "$GH_ASSETS" "$D" Lucidos_0.77.0_aarch64.dmg Lucidos.app.tar.gz.sig
out="$(release_upload_artifacts_then_manifest v0.77.0 example-org/example-repo "$D/latest.json" \
        "$D/Lucidos_0.77.0_aarch64.dmg" "$D/Lucidos.app.tar.gz" "$D/Lucidos.app.tar.gz.sig" 2>&1)"; rc=$?
if [ $rc -ne 0 ]; then
    pass "a missing artifact refuses"
else
    fail "the manifest was published over a missing payload"
fi
if grep -q 'latest.json' "$GH_CALLS"; then
    fail "latest.json was uploaded anyway: $(cat "$GH_CALLS")"
else
    pass "no latest.json upload was attempted"
fi
case "$out" in
    *"Lucidos.app.tar.gz is not on the release yet"*) pass "the refusal names the missing asset" ;;
    *) fail "the refusal does not name the missing asset: $out" ;;
esac
case "$out" in
    *v0.16.0*) pass "the refusal names the incident it exists to prevent" ;;
    *) fail "the refusal does not name the v0.16.0 window: $out" ;;
esac

echo ""
echo "test: a TRUNCATED payload also blocks the manifest"
D="$(new_release_fixture short)"
export GH_CALLS="$WORK/calls-short" GH_ASSETS="$WORK/assets-short"
: > "$GH_CALLS"
printf '{"assets":[{"name":"Lucidos_0.77.0_aarch64.dmg","size":%s,"state":"uploaded"},{"name":"Lucidos.app.tar.gz","size":1,"state":"uploaded"},{"name":"Lucidos.app.tar.gz.sig","size":%s,"state":"uploaded"}]}' \
    "$(release_upload_file_size "$D/Lucidos_0.77.0_aarch64.dmg")" \
    "$(release_upload_file_size "$D/Lucidos.app.tar.gz.sig")" > "$GH_ASSETS"
if release_upload_artifacts_then_manifest v0.77.0 example-org/example-repo "$D/latest.json" \
        "$D/Lucidos_0.77.0_aarch64.dmg" "$D/Lucidos.app.tar.gz" "$D/Lucidos.app.tar.gz.sig" >/dev/null 2>&1; then
    fail "a truncated payload still got its manifest published"
else
    pass "a truncated payload refuses"
fi
if grep -q 'latest.json' "$GH_CALLS"; then
    fail "latest.json was uploaded over a truncated payload"
else
    pass "no latest.json upload was attempted"
fi

echo ""
echo "test: a failed artifact upload never reaches the presence check"
D="$(new_release_fixture uploadfail)"
export GH_CALLS="$WORK/calls-uploadfail" GH_ASSETS="$WORK/assets-uploadfail" GH_UPLOAD_FAILS=1
: > "$GH_CALLS"
write_assets "$GH_ASSETS" "$D" Lucidos_0.77.0_aarch64.dmg Lucidos.app.tar.gz Lucidos.app.tar.gz.sig
if release_upload_artifacts_then_manifest v0.77.0 example-org/example-repo "$D/latest.json" \
        "$D/Lucidos_0.77.0_aarch64.dmg" "$D/Lucidos.app.tar.gz" "$D/Lucidos.app.tar.gz.sig" >/dev/null 2>&1; then
    fail "a failing gh upload reported success"
else
    pass "a failing artifact upload refuses"
fi
if grep -q 'release view' "$GH_CALLS"; then
    fail "the presence check ran after the upload had already failed"
else
    pass "the presence check is skipped when the upload failed"
fi
unset GH_UPLOAD_FAILS

echo ""
echo "test: a missing local file refuses before any gh call"
D="$(new_release_fixture nofile)"
export GH_CALLS="$WORK/calls-nofile" GH_ASSETS="$WORK/assets-nofile"
: > "$GH_CALLS"
: > "$GH_ASSETS"
if release_upload_artifacts_then_manifest v0.77.0 example-org/example-repo "$D/latest.json" \
        "$D/does-not-exist.dmg" >/dev/null 2>&1; then
    fail "a missing local artifact was accepted"
else
    pass "a missing local artifact refuses"
fi
if [ -s "$GH_CALLS" ]; then
    fail "gh ran despite a missing local file: $(cat "$GH_CALLS")"
else
    pass "gh was never invoked"
fi

# ── 3. latest.json, and where its platform key comes from (F10) ──────────────
# The key used to be `case "$(uname -m)"` at the call site, which describes the
# machine running the upload rather than the payload it is uploading. The
# mislabelling is silent: an updater whose target key is absent from `platforms`
# reports "no update" instead of an error, so nothing anywhere would report it.
# The key is now derived from the staged Mach-O at build time and recorded in the
# staging manifest; this generator only ever uses what it is handed.
LATEST_DIR="$WORK/latest"
mkdir -p "$LATEST_DIR"
printf 'dW50cnVzdGVkIGNvbW1lbnQ6IGZha2Ugc2ln\n' > "$LATEST_DIR/Lucidos.app.tar.gz.sig"
printf '### Added\n- A "thing" that needs\tJSON escaping\n' > "$LATEST_DIR/notes.md"
LATEST_JSON="$LATEST_DIR/latest.json"
URL="https://github.com/lucidos-dev/lucidos/releases/download/v0.77.0/Lucidos.app.tar.gz"

latest_field() {  # <python-expression over `m`>
    python3 -c "
import json, sys
m = json.load(open(sys.argv[1], encoding='utf-8'))
print($1)
" "$LATEST_JSON"
}

echo ""
echo "test: latest.json is written under the platform key it is GIVEN"
if release_upload_write_latest_json "$LATEST_JSON" 0.77.0 darwin-x86_64 \
        "$URL" 2026-08-02T11:16:04Z "$LATEST_DIR/Lucidos.app.tar.gz.sig" "$LATEST_DIR/notes.md"; then
    pass "the generator succeeds"
else
    fail "the generator returned non-zero"
fi
GOT="$(latest_field 'list(m["platforms"])[0]')"
if [ "$GOT" = "darwin-x86_64" ]; then
    pass "the platforms key is the one passed in"
else
    fail "the platforms key was '$GOT'"
fi
# The point of the whole finding: an Intel key must be producible on an Apple
# Silicon host and vice versa, because the artifact decides, not the host.
HOST_KEY="darwin-aarch64"
[ "$(uname -m)" = "x86_64" ] && HOST_KEY="darwin-x86_64"
if [ "$GOT" != "$HOST_KEY" ]; then
    pass "the key is independent of this host's own architecture ($(uname -m))"
else
    echo "  skip: this host is x86_64, so the passed key coincides with uname's answer"
fi
GOT="$(latest_field 'm["version"]')"
if [ "$GOT" = "0.77.0" ]; then pass "version is recorded"; else fail "version was '$GOT'"; fi
GOT="$(latest_field 'm["platforms"]["darwin-x86_64"]["url"]')"
if [ "$GOT" = "$URL" ]; then pass "the download url is recorded"; else fail "url was '$GOT'"; fi
GOT="$(latest_field 'm["platforms"]["darwin-x86_64"]["signature"]')"
if [ "$GOT" = "dW50cnVzdGVkIGNvbW1lbnQ6IGZha2Ugc2ln" ]; then
    pass "the signature is read from the .sig file and trimmed"
else
    fail "signature was '$GOT'"
fi
GOT="$(latest_field 'm["notes"]')"
if echo "$GOT" | grep -q 'JSON escaping'; then
    pass "the changelog notes survive JSON encoding"
else
    fail "notes were '$GOT'"
fi

echo ""
echo "test: latest.json can be written with no notes file"
rm -f "$LATEST_JSON"
if release_upload_write_latest_json "$LATEST_JSON" 0.77.0 darwin-aarch64 \
        "$URL" 2026-08-02T11:16:04Z "$LATEST_DIR/Lucidos.app.tar.gz.sig" 2>/dev/null \
   && [ "$(latest_field 'm["notes"]')" = "" ]; then
    pass "an absent --notes-file yields empty notes, not a failure"
else
    fail "the generator could not write a manifest without notes"
fi

echo ""
echo "test: the generator refuses an empty platform key rather than emitting one"
rm -f "$LATEST_JSON"
out="$(release_upload_write_latest_json "$LATEST_JSON" 0.77.0 "" \
        "$URL" 2026-08-02T11:16:04Z "$LATEST_DIR/Lucidos.app.tar.gz.sig" 2>&1)"; rc=$?
if [ $rc -ne 0 ] && echo "$out" | grep -q "no platform key"; then
    pass "an empty key is refused by name"
else
    fail "an empty key was accepted (rc=$rc): $out"
fi
if [ ! -s "$LATEST_JSON" ]; then
    pass "no manifest is left behind for the upload step to find"
else
    fail "a manifest was written despite the refusal: $(cat "$LATEST_JSON")"
fi
if echo "$out" | grep -q "no update"; then
    pass "the refusal names the silent failure it prevents"
else
    fail "the refusal does not say why an absent key is dangerous: $out"
fi

echo ""
echo "test: the generator refuses a missing signature file"
rm -f "$LATEST_JSON"
out="$(release_upload_write_latest_json "$LATEST_JSON" 0.77.0 darwin-aarch64 \
        "$URL" 2026-08-02T11:16:04Z "$LATEST_DIR/nope.sig" 2>&1)"; rc=$?
if [ $rc -ne 0 ] && echo "$out" | grep -qi "no updater signature file"; then
    pass "a missing .sig is refused by name"
else
    fail "a missing .sig was accepted (rc=$rc): $out"
fi

echo ""
echo "test: nothing on the latest.json path consults uname"
# Comments are stripped first, so the prose ABOUT the old `uname -m` derivation
# can neither satisfy nor violate the rule (the same technique the wiring checks
# below use, and front_door_gate_test.sh's reason for doing it).
UNAME_CODE="$(grep -vE '^[[:space:]]*#' "$SCRIPT_DIR/release_upload.sh" | grep -n 'uname' || true)"
if [ -z "$UNAME_CODE" ]; then
    pass "release_upload.sh never asks the host what architecture it is"
else
    fail "release_upload.sh consults uname: $UNAME_CODE"
fi

# ── 4. Wiring + public-mirror safety ─────────────────────────────────────────
echo ""
echo "test: build-dmg.sh publishes through the ordered helper"
BUILD_DMG="$PROJECT_DIR/scripts/build-dmg.sh"
if grep -q 'release_upload_artifacts_then_manifest' "$BUILD_DMG"; then
    pass "upload_staged_assets calls the ordered helper"
else
    fail "build-dmg.sh does not use release_upload_artifacts_then_manifest"
fi
# A surviving four-asset `gh release upload` would be the original bug back.
RAW_UPLOAD="$(grep -vE '^[[:space:]]*#' "$BUILD_DMG" | grep 'gh release upload' || true)"
if [ -z "$RAW_UPLOAD" ]; then
    pass "no direct 'gh release upload' remains in build-dmg.sh"
else
    fail "a direct upload bypasses the ordering: $RAW_UPLOAD"
fi

echo ""
echo "test: build-dmg.sh takes latest.json's platform key from the manifest, never uname"
if grep -q 'release_upload_write_latest_json' "$BUILD_DMG"; then
    pass "upload_staged_assets calls the shared generator"
else
    fail "build-dmg.sh does not use release_upload_write_latest_json"
fi
# The trailing space is what distinguishes the manifest READER from
# release_staging_platform_key_for_binary, which derives the key at build time
# and must not be what the upload path consults.
if grep -qE 'release_staging_platform_key[[:space:]]' "$BUILD_DMG"; then
    pass "the key is read back from the staging manifest"
else
    fail "build-dmg.sh does not read platform_key from the staging manifest"
fi
# The original bug, in one grep. `uname -m` still appears in build-dmg.sh for the
# BUILD host's target triple, which is a different and legitimate question, so
# this is scoped to the latest.json platform keys rather than to uname itself.
PLATFORM_UNAME="$(grep -vE '^[[:space:]]*#' "$BUILD_DMG" \
    | grep -nE 'darwin-(aarch64|x86_64)' || true)"
if [ -z "$PLATFORM_UNAME" ]; then
    pass "no latest.json platform key is spelled out in build-dmg.sh at all"
else
    fail "a hardcoded platform key survives in build-dmg.sh: $PLATFORM_UNAME"
fi

echo ""
echo "test: the lib ships to the public mirror"
TREE_LIB="$PROJECT_DIR/scripts/lib/release_tree.sh"
if [ ! -f "$TREE_LIB" ]; then
    echo "  skip: release_tree.sh is not present (stripped from the public mirror)"
else
    if grep -q 'release_upload' "$TREE_LIB"; then
        fail "release_upload.sh is withheld from the public tree but sourced unconditionally"
    else
        pass "release_upload.sh is not in RELEASE_TREE_EXCLUDE_PATHS"
    fi
fi

echo ""
echo "release_upload: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
