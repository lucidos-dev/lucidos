#!/usr/bin/env bash
# Tests for release.sh's compiled-input re-fold gate — refold_can_reuse_staged_build()
# and restage_manifest_for_commit().
#
# This is the integration-level half of the 2026-07-28 fix. The unit tests in
# release_build_fingerprint_test.sh prove the fingerprint; these prove the GATE
# built on it makes the right call against a real git repo and a real staging
# dir: a docs-only commit reuses the staged notarized DMG, a source change
# rebuilds, a version bump rebuilds even on identical source, and a staging dir
# written before the gate existed rebuilds rather than silently reusing.
#
# The two functions live in release.sh (a script, not a library), so they are
# extracted with awk — the same technique build_dmg_test.sh uses. Pure git +
# python3: no build, no xcrun, no network.
# Run: ./scripts/lib/release_refold_gate_test.sh
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
# shellcheck source=scripts/lib/release_staging.sh
source "$SCRIPT_DIR/release_staging.sh"
# shellcheck source=scripts/lib/release_build_fingerprint.sh
source "$SCRIPT_DIR/release_build_fingerprint.sh"

PASS=0
FAIL=0
fail() { echo "  FAIL: $*"; FAIL=$((FAIL+1)); }
pass() { echo "  ok:   $*"; PASS=$((PASS+1)); }

# Extract the gate + restamp helpers from release.sh.
EXTRACT="$(mktemp)"
awk '/^refold_can_reuse_staged_build\(\) \{/,/^\}/' "$PROJECT_DIR/scripts/release.sh" >  "$EXTRACT"
awk '/^restage_manifest_for_commit\(\) \{/,/^\}/'   "$PROJECT_DIR/scripts/release.sh" >> "$EXTRACT"
[ -s "$EXTRACT" ] || { echo "  FAIL: could not extract the gate from release.sh"; exit 1; }
# shellcheck source=/dev/null
source "$EXTRACT"
rm -f "$EXTRACT"

# A repo shaped like the real one + a staging dir standing in for a completed
# Phase A (fake artifact bytes; the gate only reads the manifest and checksums).
new_case() {
    local dir
    dir="$(mktemp -d)"
    git -C "$dir" init --quiet
    git -C "$dir" config user.email t@example.com
    git -C "$dir" config user.name Test
    mkdir -p "$dir/crates/lucidos-engine/src" "$dir/packages" "$dir/system-knowhow" "$dir/scripts/lib"
    printf 'fn main() {}\n' > "$dir/crates/lucidos-engine/src/main.rs"
    printf 'lock\n'         > "$dir/Cargo.lock"
    printf '[workspace]\n'  > "$dir/Cargo.toml"
    printf '{}\n'           > "$dir/package.json"
    printf '{}\n'           > "$dir/package-lock.json"
    printf 'sdk\n'          > "$dir/packages/sdk.ts"
    printf 'kh\n'           > "$dir/system-knowhow/g.md"
    printf '#!/bin/sh\n'    > "$dir/scripts/build-dmg.sh"
    printf '#!/bin/sh\n'    > "$dir/scripts/lib/stage_runtime.sh"
    printf '0.77.0\n'       > "$dir/RELEASE"
    printf '# cl\n'         > "$dir/CHANGELOG.md"
    printf '# contrib\n'    > "$dir/CONTRIBUTING.md"
    printf '#!/bin/sh\n'    > "$dir/install.sh"
    git -C "$dir" add -A >/dev/null
    git -C "$dir" commit --quiet -m "base"
    printf '%s' "$dir"
}

# Stage a "completed Phase A" for the repo at its current HEAD.
stage_at_head() {
    local repo="$1" staging="$2" commit fp recipe
    commit="$(git -C "$repo" rev-parse HEAD)"
    mkdir -p "$staging"
    printf 'notarized dmg bytes\n' > "$staging/Lucidos_0.77.0_aarch64.dmg"
    printf 'updater\n'             > "$staging/Lucidos.app.tar.gz"
    printf 'sig\n'                 > "$staging/Lucidos.app.tar.gz.sig"
    fp="$(release_build_fingerprint_compute "$repo" "$commit")"
    recipe="$(release_build_recipe_fingerprint_compute "$repo" "$commit")"
    RELEASE_STAGING_BUILD_FINGERPRINT="$fp" \
    RELEASE_STAGING_RECIPE_FINGERPRINT="$recipe" \
    release_staging_write_manifest "$staging" "0.77.0" "$commit" \
        "Lucidos_0.77.0_aarch64.dmg" "Lucidos.app.tar.gz" "Lucidos.app.tar.gz.sig"
}

commit_all() { git -C "$1" add -A >/dev/null; git -C "$1" commit --quiet -m "$2"; }

# ── 1. THE INCIDENT: a docs-only re-fold reuses the notarized DMG ────────────
echo "test: a docs-only re-fold REUSES the staged build (no rebuild, no resubmit)"
REPO="$(new_case)"
# shellcheck disable=SC2034 # global read by refold_can_reuse_staged_build
REPO_ROOT="$REPO"
# shellcheck disable=SC2034 # global read by refold_can_reuse_staged_build
VERSION="0.77.0"
STAGING_DIR="$REPO/.lucidos/release-staging/0.77.0"
stage_at_head "$REPO" "$STAGING_DIR"
DMG_SHA_BEFORE="$(release_staging_sha256 "$STAGING_DIR/Lucidos_0.77.0_aarch64.dmg")"
printf '# contributing, rewritten\n' > "$REPO/CONTRIBUTING.md"
printf '#!/bin/sh\n# no race\n'      > "$REPO/install.sh"
commit_all "$REPO" "docs + install fixes"
NEW_HEAD="$(git -C "$REPO" rev-parse HEAD)"
if refold_can_reuse_staged_build "$NEW_HEAD" >/dev/null 2>&1; then
    pass "gate says REUSE"
else
    fail "gate forced a rebuild for a docs-only re-fold"
fi
# …and the restamp keeps the artifacts byte-identical while moving provenance.
if restage_manifest_for_commit "$STAGING_DIR" "$NEW_HEAD" >/dev/null 2>&1; then
    pass "manifest restamped"
else
    fail "restamp failed"
fi
if [ "$(release_staging_manifest_field "$STAGING_DIR" source_commit)" = "$NEW_HEAD" ]; then
    pass "source_commit now names the new release commit"
else
    fail "source_commit was not advanced"
fi
if [ "$(release_staging_sha256 "$STAGING_DIR/Lucidos_0.77.0_aarch64.dmg")" = "$DMG_SHA_BEFORE" ]; then
    pass "the notarized DMG bytes are untouched"
else
    fail "the DMG changed during a reuse"
fi
if release_staging_verify "$STAGING_DIR" >/dev/null 2>&1; then
    pass "staging still verifies after the restamp"
else
    fail "the restamped staging fails verification"
fi
rm -rf "$REPO"

# ── 2. a real source change rebuilds ─────────────────────────────────────────
echo "test: a crates/ change forces a rebuild"
REPO="$(new_case)"
# shellcheck disable=SC2034 # global read by refold_can_reuse_staged_build
REPO_ROOT="$REPO"
# shellcheck disable=SC2034 # global read by refold_can_reuse_staged_build
VERSION="0.77.0"
STAGING_DIR="$REPO/.lucidos/release-staging/0.77.0"
stage_at_head "$REPO" "$STAGING_DIR"
printf 'fn main() { println!("new"); }\n' > "$REPO/crates/lucidos-engine/src/main.rs"
commit_all "$REPO" "feat: change the engine"
if refold_can_reuse_staged_build "$(git -C "$REPO" rev-parse HEAD)" >/dev/null 2>&1; then
    fail "gate reused a staged build after a real source change"
else
    pass "gate says REBUILD"
fi
rm -rf "$REPO"

# ── 3. THE VERSION TRAP: identical source, bumped version ⇒ rebuild ──────────
# The version is compiled into the app and stamped into the DMG name, so a
# staged 0.77.0 DMG is stale for 0.78.0 no matter how identical the source is.
echo "test: a version bump rebuilds even when the source is byte-identical"
REPO="$(new_case)"; REPO_ROOT="$REPO"; VERSION="0.78.0"
STAGING_DIR="$REPO/.lucidos/release-staging/0.77.0"
stage_at_head "$REPO" "$STAGING_DIR"     # staged as 0.77.0
printf '0.78.0\n' > "$REPO/RELEASE"
commit_all "$REPO" "Release v0.78.0"
if refold_can_reuse_staged_build "$(git -C "$REPO" rev-parse HEAD)" >/dev/null 2>&1; then
    fail "gate reused a 0.77.0 DMG for a 0.78.0 release"
else
    pass "gate says REBUILD across a version bump"
fi
rm -rf "$REPO"

# ── 4. a pre-gate staging dir rebuilds (absent ≠ unchanged) ──────────────────
echo "test: staging written before the gate existed forces a rebuild"
REPO="$(new_case)"
# shellcheck disable=SC2034 # global read by refold_can_reuse_staged_build
REPO_ROOT="$REPO"
# shellcheck disable=SC2034 # global read by refold_can_reuse_staged_build
VERSION="0.77.0"
STAGING_DIR="$REPO/.lucidos/release-staging/0.77.0"
mkdir -p "$STAGING_DIR"
printf 'dmg\n' > "$STAGING_DIR/Lucidos_0.77.0_aarch64.dmg"
printf 'tgz\n' > "$STAGING_DIR/Lucidos.app.tar.gz"
printf 'sig\n' > "$STAGING_DIR/Lucidos.app.tar.gz.sig"
release_staging_write_manifest "$STAGING_DIR" "0.77.0" "$(git -C "$REPO" rev-parse HEAD)" \
    "Lucidos_0.77.0_aarch64.dmg" "Lucidos.app.tar.gz" "Lucidos.app.tar.gz.sig"   # no fingerprints
if refold_can_reuse_staged_build "$(git -C "$REPO" rev-parse HEAD)" >/dev/null 2>&1; then
    fail "a fingerprint-less manifest was treated as 'unchanged'"
else
    pass "no recorded fingerprint ⇒ rebuild (fails closed)"
fi
rm -rf "$REPO"

# ── 5. corrupt / missing staging rebuilds ────────────────────────────────────
echo "test: unverifiable staging forces a rebuild"
REPO="$(new_case)"
# shellcheck disable=SC2034 # global read by refold_can_reuse_staged_build
REPO_ROOT="$REPO"
# shellcheck disable=SC2034 # global read by refold_can_reuse_staged_build
VERSION="0.77.0"
STAGING_DIR="$REPO/.lucidos/release-staging/0.77.0"
stage_at_head "$REPO" "$STAGING_DIR"
printf 'TAMPERED\n' > "$STAGING_DIR/Lucidos_0.77.0_aarch64.dmg"
if refold_can_reuse_staged_build "$(git -C "$REPO" rev-parse HEAD)" >/dev/null 2>&1; then
    fail "a checksum-mismatched staging dir was reused"
else
    pass "tampered staging ⇒ rebuild"
fi
rm -rf "$REPO"

echo "test: a missing staging dir forces a rebuild"
REPO="$(new_case)"
# shellcheck disable=SC2034 # global read by refold_can_reuse_staged_build
REPO_ROOT="$REPO"
# shellcheck disable=SC2034 # global read by refold_can_reuse_staged_build
VERSION="0.77.0"
STAGING_DIR="$REPO/.lucidos/release-staging/nope"
if refold_can_reuse_staged_build "$(git -C "$REPO" rev-parse HEAD)" >/dev/null 2>&1; then
    fail "a missing staging dir was reused"
else
    pass "no staging ⇒ rebuild"
fi
rm -rf "$REPO"

# ── 6. a build-recipe change rebuilds, and says so distinctly ────────────────
echo "test: a build-recipe change rebuilds and is reported as such"
REPO="$(new_case)"
# shellcheck disable=SC2034 # global read by refold_can_reuse_staged_build
REPO_ROOT="$REPO"
# shellcheck disable=SC2034 # global read by refold_can_reuse_staged_build
VERSION="0.77.0"
STAGING_DIR="$REPO/.lucidos/release-staging/0.77.0"
stage_at_head "$REPO" "$STAGING_DIR"
printf '#!/bin/sh\n# recipe changed\n' > "$REPO/scripts/build-dmg.sh"
commit_all "$REPO" "change the bundler"
rc=0
OUT="$(refold_can_reuse_staged_build "$(git -C "$REPO" rev-parse HEAD)" 2>&1)" || rc=$?
if [ "$rc" -ne 0 ]; then
    pass "recipe change ⇒ rebuild"
else
    fail "a recipe change was reused"
fi
case "$OUT" in
    *"build recipe moved"*) pass "reported as a recipe change, not a source change" ;;
    *)                      fail "unclear recipe message: '$OUT'" ;;
esac
case "$OUT" in
    *build-dmg.sh*) pass "names the recipe file that moved" ;;
    *)              fail "did not name build-dmg.sh: '$OUT'" ;;
esac
rm -rf "$REPO"

echo ""
echo "release_refold_gate: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
