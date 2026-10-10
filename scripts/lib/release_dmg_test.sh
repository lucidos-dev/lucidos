#!/usr/bin/env bash
# Tests for scripts/lib/release_dmg.sh, the "which file is the release DMG"
# discovery. Pure path arithmetic plus `find` over fixture files, so the whole
# suite runs offline on any host: no hdiutil, no build, no Apple credentials.
# Run: ./scripts/lib/release_dmg_test.sh
#
# THE BUG THIS PINS DOWN (F4 in docs/audits/2026-08-02-macos-update-path-audit.md).
# refresh_dmg_payload writes `.rw.dmg` and `.zlib.dmg` next to the real artifact,
# a run killed mid-refresh leaves one behind, and build-dmg.sh's main discovery
# was `find … -name '*.dmg' | head -1` with no exclusion and no arity check. The
# version-stamp guard could not catch it either, because the leftovers carry the
# same version string. The adopt path already had both checks, which is why the
# drift assertion below matters more than any single case: the writer and the
# exclusion must keep agreeing.
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
# shellcheck source=scripts/lib/release_dmg.sh
source "$SCRIPT_DIR/release_dmg.sh"

PASS=0
FAIL=0
fail() { echo "  FAIL: $*"; FAIL=$((FAIL+1)); }
pass() { echo "  ok:   $*"; PASS=$((PASS+1)); }

WORK="$(mktemp -d)"
cleanup() { rm -rf "$WORK"; }
trap cleanup EXIT

REAL_NAME="Lucidos_0.77.0_aarch64.dmg"   # synthetic version: never a real release

new_dir() {  # <name>
    local d="$WORK/$1"
    mkdir -p "$d"
    printf '%s' "$d"
}

# ── 1. The intermediate paths ────────────────────────────────────────────────
echo "test: the intermediate paths compose the names refresh_dmg_payload writes"
D="/tmp/bundle/dmg/$REAL_NAME"
got="$(release_dmg_rw_path "$D")"
if [ "$got" = "/tmp/bundle/dmg/Lucidos_0.77.0_aarch64.rw.dmg" ]; then
    pass "rw_path replaces the .dmg suffix"
else
    fail "rw_path returned '$got'"
fi
got="$(release_dmg_zlib_path "$D")"
if [ "$got" = "/tmp/bundle/dmg/Lucidos_0.77.0_aarch64.zlib.dmg" ]; then
    pass "zlib_path replaces the .dmg suffix"
else
    fail "zlib_path returned '$got'"
fi

# THE DRIFT ASSERTION. F4 exists because one site knew the suffixes and another
# did not. The predicate must recognise exactly what the writer helpers produce,
# so adding a third intermediate cannot leave the exclusion behind.
echo ""
echo "test: the exclusion recognises exactly what the writer helpers produce"
for helper in release_dmg_rw_path release_dmg_zlib_path; do
    if release_dmg_is_intermediate "$("$helper" "$D")"; then
        pass "$helper's output is recognised as an intermediate"
    else
        fail "$helper's output is NOT excluded by release_dmg_is_intermediate"
    fi
done
if release_dmg_is_intermediate "$D"; then
    fail "the real DMG was classified as an intermediate"
else
    pass "the real DMG is not an intermediate"
fi
# Matched on the basename, so a parent directory cannot poison the verdict.
if release_dmg_is_intermediate "/tmp/weird.rw.dmg/$REAL_NAME"; then
    fail "a parent directory ending in .rw.dmg made a real artifact look like a leftover"
else
    pass "the match is on the basename, not anywhere in the path"
fi

# ── 2. Discovery ─────────────────────────────────────────────────────────────
echo ""
echo "test: discovery ignores the intermediates and returns the real DMG"
DIR="$(new_dir leftovers)"
printf 'real\n'         > "$DIR/$REAL_NAME"
printf 'rw leftover\n'  > "$DIR/Lucidos_0.77.0_aarch64.rw.dmg"
printf 'zlib leftover\n'> "$DIR/Lucidos_0.77.0_aarch64.zlib.dmg"
got="$(release_dmg_find "$DIR" 2>&1)"; rc=$?
if [ $rc -eq 0 ] && [ "$got" = "$DIR/$REAL_NAME" ]; then
    pass "a directory holding both leftovers still yields the real DMG"
else
    fail "discovery returned '$got' (rc=$rc)"
fi

echo ""
echo "test: two real candidates are refused rather than guessed between"
printf 'other arch\n' > "$DIR/Lucidos_0.77.0_x86_64.dmg"
out="$(release_dmg_find "$DIR" 2>&1)"; rc=$?
if [ $rc -ne 0 ] && echo "$out" | grep -qi "candidate DMGs"; then
    pass "an ambiguous directory refuses"
else
    fail "expected an ambiguity refusal; got rc=$rc out: $out"
fi
if echo "$out" | grep -q "Lucidos_0.77.0_aarch64.dmg" && echo "$out" | grep -q "Lucidos_0.77.0_x86_64.dmg"; then
    pass "the refusal names both candidates"
else
    fail "the refusal does not name both candidates: $out"
fi
if echo "$out" | grep -q "rw.dmg\|zlib.dmg"; then
    fail "the intermediates were counted as candidates: $out"
else
    pass "the intermediates are not counted toward the ambiguity"
fi

echo ""
echo "test: a directory holding ONLY intermediates refuses and says so"
DIR="$(new_dir only-leftovers)"
printf 'rw leftover\n' > "$DIR/Lucidos_0.77.0_aarch64.rw.dmg"
out="$(release_dmg_find "$DIR" 2>&1)"; rc=$?
if [ $rc -ne 0 ] && echo "$out" | grep -qi "no .dmg found"; then
    pass "only-intermediates refuses"
else
    fail "expected a no-candidate refusal; got rc=$rc out: $out"
fi
if echo "$out" | grep -qi "killed mid-refresh"; then
    pass "the refusal explains where the leftovers came from"
else
    fail "the refusal does not explain the leftovers: $out"
fi

echo ""
echo "test: an empty directory, a missing directory and no argument all refuse"
DIR="$(new_dir empty)"
out="$(release_dmg_find "$DIR" 2>&1)"; rc=$?
if [ $rc -ne 0 ] && echo "$out" | grep -qi "no .dmg found"; then
    pass "an empty directory refuses"
else
    fail "expected a no-candidate refusal for an empty dir; got rc=$rc out: $out"
fi
out="$(release_dmg_find "$WORK/does-not-exist" 2>&1)"; rc=$?
if [ $rc -ne 0 ] && echo "$out" | grep -qi "no DMG output directory"; then
    pass "a missing directory refuses"
else
    fail "expected a missing-directory refusal; got rc=$rc out: $out"
fi
out="$(release_dmg_find "" 2>&1)"; rc=$?
if [ $rc -ne 0 ]; then
    pass "an empty argument refuses"
else
    fail "release_dmg_find accepted an empty directory argument"
fi

echo ""
echo "test: discovery is depth-1 and files-only"
DIR="$(new_dir depth)"
mkdir -p "$DIR/nested" "$DIR/Lucidos_0.77.0_staging.dmg"
printf 'real\n'   > "$DIR/$REAL_NAME"
printf 'nested\n' > "$DIR/nested/Lucidos_0.77.0_nested.dmg"
got="$(release_dmg_find "$DIR" 2>&1)"; rc=$?
if [ $rc -eq 0 ] && [ "$got" = "$DIR/$REAL_NAME" ]; then
    pass "a nested .dmg and a directory named *.dmg are both ignored"
else
    fail "depth/type filtering failed: rc=$rc got '$got'"
fi

# ── 3. Wiring + public-mirror safety ─────────────────────────────────────────
echo ""
echo "test: build-dmg.sh uses the shared discovery at every site"
BUILD_DMG="$PROJECT_DIR/scripts/build-dmg.sh"
# shellcheck disable=SC2016 # matching the literal source text, not expanding it
if grep -q 'source "\$SCRIPT_DIR/lib/release_dmg.sh"' "$BUILD_DMG"; then
    pass "build-dmg.sh sources the lib unconditionally"
else
    fail "build-dmg.sh does not source release_dmg.sh unconditionally"
fi
# Both discovery sites (the main build and the adopt path) must go through it,
# or the exclusion exists in one place again, which IS the finding.
FIND_COUNT="$(grep -c 'release_dmg_find' "$BUILD_DMG")"
if [ "$FIND_COUNT" -ge 2 ]; then
    pass "release_dmg_find is used at $FIND_COUNT sites"
else
    fail "expected both DMG discovery sites to use release_dmg_find, found $FIND_COUNT"
fi
# A raw `find … -name '*.dmg'` in build-dmg.sh would be a discovery that skipped
# the exclusion. Comment lines are dropped first: the header prose quotes the old
# broken command deliberately, to record what it was.
RAW_FIND="$(grep -vE '^[[:space:]]*#' "$BUILD_DMG" | grep -n "find .*-name '\*\.dmg'" || true)"
if [ -z "$RAW_FIND" ]; then
    pass "no raw '*.dmg' find survives in build-dmg.sh"
else
    fail "a raw '*.dmg' find bypasses the exclusion: $RAW_FIND"
fi
# The intermediate paths refresh_dmg_payload writes must come from the helpers,
# not from a second copy of the suffix strings.
REFRESH_FN="$(awk '/^refresh_dmg_payload\(\) \{/,/^\}/' "$BUILD_DMG")"
if printf '%s\n' "$REFRESH_FN" | grep -q 'release_dmg_rw_path' \
   && printf '%s\n' "$REFRESH_FN" | grep -q 'release_dmg_zlib_path'; then
    pass "refresh_dmg_payload takes its intermediate paths from the shared helpers"
else
    fail "refresh_dmg_payload still spells the intermediate suffixes itself"
fi

echo ""
echo "test: the lib ships to the public mirror"
# build-dmg.sh --release-build is a legitimate public path and sources this
# unconditionally, so withholding it would break a mirror clone.
TREE_LIB="$PROJECT_DIR/scripts/lib/release_tree.sh"
if [ ! -f "$TREE_LIB" ]; then
    echo "  skip: release_tree.sh is not present (stripped from the public mirror)"
else
    if grep -q 'release_dmg' "$TREE_LIB"; then
        fail "release_dmg.sh is withheld from the public tree but sourced unconditionally"
    else
        pass "release_dmg.sh is not in RELEASE_TREE_EXCLUDE_PATHS"
    fi
fi

echo ""
echo "release_dmg: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
