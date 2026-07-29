#!/usr/bin/env bash
# Tests for scripts/lib/release_build_fingerprint.sh — the compiled-input
# fingerprint that stops a docs-only commit from forcing a rebuild + a fresh
# Apple notarization submission (the 2026-07-28 incident: 7 submissions for one
# release, 3 of them byte-identical compiled input).
#
# Pure git plumbing against throwaway repos in mktemp dirs — no network, no
# xcrun, no cargo. Run: ./scripts/lib/release_build_fingerprint_test.sh
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/lib/release_build_fingerprint.sh
source "$SCRIPT_DIR/release_build_fingerprint.sh"

PASS=0
FAIL=0
fail() { echo "  FAIL: $*"; FAIL=$((FAIL+1)); }
pass() { echo "  ok:   $*"; PASS=$((PASS+1)); }

# A throwaway repo shaped like the real one: the tracked compiled-input paths
# plus the untracked-by-the-fingerprint files whose changes caused the incident.
new_repo() {
    local dir
    dir="$(mktemp -d)"
    git -C "$dir" init --quiet
    git -C "$dir" config user.email t@example.com
    git -C "$dir" config user.name  Test

    mkdir -p "$dir/crates/lucidos-engine/src" "$dir/crates/lucidos-app/src" \
             "$dir/packages/lucidos-sdk/src" "$dir/system-knowhow" \
             "$dir/scripts/lib" "$dir/docs"
    printf 'fn main() {}\n'        > "$dir/crates/lucidos-engine/src/main.rs"
    printf 'export const x = 1;\n' > "$dir/crates/lucidos-app/src/App.tsx"
    printf 'export const s = 1;\n' > "$dir/packages/lucidos-sdk/src/index.ts"
    printf '# knowhow\n'           > "$dir/system-knowhow/guide.md"
    printf 'lock\n'                > "$dir/Cargo.lock"
    printf '[workspace]\n'         > "$dir/Cargo.toml"
    printf '{}\n'                  > "$dir/package.json"
    printf '{}\n'                  > "$dir/package-lock.json"
    printf '#!/bin/sh\n'           > "$dir/scripts/build-dmg.sh"
    printf '#!/bin/sh\n'           > "$dir/scripts/lib/stage_runtime.sh"
    # Not fingerprinted — the incident's actual culprits.
    # Synthetic: a fixture version must never collide with a real release
    # version, or version_sources_test.sh reads it as an unmanaged literal.
    printf '0.77.0\n'              > "$dir/RELEASE"
    printf '# changelog\n'         > "$dir/CHANGELOG.md"
    printf '# contributing\n'      > "$dir/CONTRIBUTING.md"
    printf '#!/bin/sh\n'           > "$dir/install.sh"
    printf '# plan\n'              > "$dir/docs/plan.md"

    git -C "$dir" add -A >/dev/null
    git -C "$dir" commit --quiet -m "base"
    printf '%s' "$dir"
}

commit_all() {
    git -C "$1" add -A >/dev/null
    git -C "$1" commit --quiet -m "$2"
}

# ── 1. identical tree ⇒ identical fingerprint (and it is stable) ─────────────
echo "test: the same tree fingerprints identically (stable, reproducible)"
REPO="$(new_repo)"
FP1="$(release_build_fingerprint_compute "$REPO")"
FP2="$(release_build_fingerprint_compute "$REPO")"
if [ -n "$FP1" ] && [ "$FP1" = "$FP2" ]; then
    pass "stable fingerprint: $FP1"
else
    fail "unstable or empty: '$FP1' vs '$FP2'"
fi
case "$FP1" in
    v1:*) pass "carries the v1: schema prefix" ;;
    *)    fail "missing the v1: prefix: $FP1" ;;
esac
rm -rf "$REPO"

# ── 2. THE INCIDENT: docs-only commits must NOT change the fingerprint ───────
# These are the five real commits that forced three redundant notarizations.
echo "test: docs / install.sh / non-lib script commits do not change the fingerprint"
REPO="$(new_repo)"
BEFORE="$(release_build_fingerprint_compute "$REPO")"

printf '# contributing, honestly\n' > "$REPO/CONTRIBUTING.md"
commit_all "$REPO" "docs: describe the publishing-mirror contribution flow"

printf '# contributing, unversioned\n' > "$REPO/CONTRIBUTING.md"
commit_all "$REPO" "docs: stop CONTRIBUTING pinning a version line"

printf '#!/bin/sh\n# version source test\n' > "$REPO/scripts/lib/version_sources_test.sh"
commit_all "$REPO" "test(release): enforce one source of truth for the version"

printf '#!/bin/sh\n# no race\n' > "$REPO/install.sh"
commit_all "$REPO" "fix(install): don't race the postgres init server"

printf '# plan v2\n' > "$REPO/docs/plan.md"
commit_all "$REPO" "docs: plan"

AFTER="$(release_build_fingerprint_compute "$REPO")"
if [ "$BEFORE" = "$AFTER" ]; then
    pass "five docs/script/install commits ⇒ fingerprint unchanged (no rebuild)"
else
    fail "docs-only commits changed the fingerprint: $BEFORE -> $AFTER"
fi
rm -rf "$REPO"

# ── 3. a crates/ change MUST change the fingerprint ──────────────────────────
echo "test: a crates/ source change changes the fingerprint"
REPO="$(new_repo)"
BEFORE="$(release_build_fingerprint_compute "$REPO")"
BEFORE_REV="$(git -C "$REPO" rev-parse HEAD)"
printf 'fn main() { println!("changed"); }\n' > "$REPO/crates/lucidos-engine/src/main.rs"
commit_all "$REPO" "feat(engine): change behaviour"
AFTER="$(release_build_fingerprint_compute "$REPO")"
if [ "$BEFORE" != "$AFTER" ]; then
    pass "crates/ change ⇒ fingerprint differs (rebuild)"
else
    fail "crates/ change did NOT change the fingerprint"
fi
EXPLAIN="$(release_build_fingerprint_explain "$REPO" "$BEFORE_REV" HEAD)"
case "$EXPLAIN" in
    *crates*) pass "explain names the crates path" ;;
    *)        fail "explain did not name crates: '$EXPLAIN'" ;;
esac
rm -rf "$REPO"

# ── 4. every other tracked path is load-bearing ──────────────────────────────
echo "test: each tracked compiled-input path changes the fingerprint"
for target in Cargo.lock Cargo.toml package.json package-lock.json \
              packages/lucidos-sdk/src/index.ts system-knowhow/guide.md \
              crates/lucidos-app/src/App.tsx; do
    REPO="$(new_repo)"
    BEFORE="$(release_build_fingerprint_compute "$REPO")"
    printf 'mutated\n' >> "$REPO/$target"
    commit_all "$REPO" "change $target"
    AFTER="$(release_build_fingerprint_compute "$REPO")"
    if [ "$BEFORE" != "$AFTER" ]; then
        pass "$target ⇒ fingerprint differs"
    else
        fail "$target did NOT change the fingerprint"
    fi
    rm -rf "$REPO"
done

# ── 5. RELEASE / CHANGELOG are deliberately excluded ─────────────────────────
echo "test: RELEASE and CHANGELOG.md are excluded (they change every release)"
REPO="$(new_repo)"
BEFORE="$(release_build_fingerprint_compute "$REPO")"
printf '0.78.0\n'                 > "$REPO/RELEASE"
printf '# changelog\n## v0.78.0\n'> "$REPO/CHANGELOG.md"
commit_all "$REPO" "Release v0.78.0"
AFTER="$(release_build_fingerprint_compute "$REPO")"
if [ "$BEFORE" = "$AFTER" ]; then
    pass "RELEASE/CHANGELOG bump ⇒ fingerprint unchanged (version guard covers it)"
else
    fail "RELEASE/CHANGELOG changed the fingerprint: $BEFORE -> $AFTER"
fi
rm -rf "$REPO"

# ── 6. THE VERSION TRAP: same fingerprint + different version ⇒ rebuild ──────
echo "test: the gate refuses a match when the target version changed"
FP="v1:$(printf 'x' | shasum -a 256 | cut -d' ' -f1)"
if release_build_fingerprint_matches "$FP" "0.77.0" "$FP" "0.77.0" 2>/dev/null; then
    pass "same fingerprint + same version ⇒ skip rebuild"
else
    fail "identical fingerprint+version was refused"
fi
if release_build_fingerprint_matches "$FP" "0.77.0" "$FP" "0.78.0" 2>/dev/null; then
    fail "identical fingerprint but DIFFERENT version was allowed to skip — the version is compiled in"
else
    pass "same fingerprint + bumped version ⇒ rebuild (version is compiled into the app)"
fi
REASON="$(release_build_fingerprint_matches "$FP" "0.77.0" "$FP" "0.78.0" 2>&1 || true)"
case "$REASON" in
    *"version changed"*) pass "refusal explains the version trap" ;;
    *)                   fail "unclear version refusal: '$REASON'" ;;
esac

# ── 7. missing fingerprint must never read as "unchanged" ────────────────────
echo "test: an absent fingerprint fails closed (rebuild), never silently matches"
if release_build_fingerprint_matches "" "0.77.0" "$FP" "0.77.0" 2>/dev/null; then
    fail "empty staged fingerprint was treated as a match"
else
    pass "empty staged fingerprint ⇒ rebuild"
fi
if release_build_fingerprint_matches "$FP" "0.77.0" "" "0.77.0" 2>/dev/null; then
    fail "empty candidate fingerprint was treated as a match"
else
    pass "empty candidate fingerprint ⇒ rebuild"
fi
if release_build_fingerprint_matches "" "" "" "" 2>/dev/null; then
    fail "all-empty was treated as a match"
else
    pass "all-empty ⇒ rebuild"
fi
if release_build_fingerprint_matches "$FP" "" "$FP" "0.77.0" 2>/dev/null; then
    fail "missing staged version was treated as a match"
else
    pass "missing version on either side ⇒ rebuild"
fi

# ── 8. differing fingerprints never match ────────────────────────────────────
echo "test: differing fingerprints ⇒ rebuild"
if release_build_fingerprint_matches "v1:aaa" "0.77.0" "v1:bbb" "0.77.0" 2>/dev/null; then
    fail "different fingerprints were treated as a match"
else
    pass "different fingerprints ⇒ rebuild"
fi

# ── 9. a bad revision fails loudly ───────────────────────────────────────────
echo "test: an unknown revision is an error, not an empty fingerprint"
REPO="$(new_repo)"
if release_build_fingerprint_compute "$REPO" "nope-not-a-rev" >/dev/null 2>&1; then
    fail "an unknown revision produced a fingerprint"
else
    pass "unknown revision ⇒ non-zero"
fi
rm -rf "$REPO"

# ── 10. TIER 2: the build recipe is tracked separately ───────────────────────
# Verified against the real incident: e4a32b901 changed ONE COMMENT in
# build-dmg.sh. In the content tier that would force a rebuild + a fresh notary
# submission for a comment — the exact waste this gate exists to stop.
echo "test: build-recipe scripts are tier 2, not part of the content fingerprint"
REPO="$(new_repo)"
CONTENT_BEFORE="$(release_build_fingerprint_compute "$REPO")"
RECIPE_BEFORE="$(release_build_recipe_fingerprint_compute "$REPO")"
BEFORE_REV="$(git -C "$REPO" rev-parse HEAD)"
printf '#!/bin/sh\n# the 2026-07-28 incident\n' > "$REPO/scripts/build-dmg.sh"
commit_all "$REPO" "test(release): keep comments off real release versions"
CONTENT_AFTER="$(release_build_fingerprint_compute "$REPO")"
RECIPE_AFTER="$(release_build_recipe_fingerprint_compute "$REPO")"
if [ "$CONTENT_BEFORE" = "$CONTENT_AFTER" ]; then
    pass "a build-dmg.sh comment does NOT move the content fingerprint"
else
    fail "a build-dmg.sh comment moved the content fingerprint"
fi
if [ "$RECIPE_BEFORE" != "$RECIPE_AFTER" ]; then
    pass "…but it DOES move the recipe fingerprint (never silently ignored)"
else
    fail "the recipe fingerprint missed a build-dmg.sh change"
fi
EXPLAIN="$(release_build_recipe_explain "$REPO" "$BEFORE_REV" HEAD)"
case "$EXPLAIN" in
    *build-dmg.sh*) pass "recipe explain names build-dmg.sh" ;;
    *)              fail "recipe explain did not name the file: '$EXPLAIN'" ;;
esac
# stage_runtime.sh is the other tier-2 path
RECIPE_B2="$(release_build_recipe_fingerprint_compute "$REPO")"
printf '#!/bin/sh\n# changed\n' > "$REPO/scripts/lib/stage_runtime.sh"
commit_all "$REPO" "change the assemble recipe"
if [ "$RECIPE_B2" != "$(release_build_recipe_fingerprint_compute "$REPO")" ]; then
    pass "stage_runtime.sh is tracked in the recipe tier"
else
    fail "stage_runtime.sh change was not detected"
fi
rm -rf "$REPO"

# ── 11. the tri-state gate ───────────────────────────────────────────────────
echo "test: the gate returns 2 (recipe changed) distinctly from 0 and 1"
rc=0
release_build_fingerprint_matches "v1:aaa" "0.77.0" "v1:aaa" "0.77.0" "v1:r1" "v1:r1" 2>/dev/null || rc=$?
if [ "$rc" -eq 0 ]; then
    pass "content+version+recipe identical ⇒ 0 (skip)"
else
    fail "full match did not return 0"
fi

rc=0
release_build_fingerprint_matches "v1:aaa" "0.77.0" "v1:aaa" "0.77.0" "v1:r1" "v1:r2" 2>/dev/null || rc=$?
if [ "$rc" -eq 2 ]; then
    pass "content identical but recipe changed ⇒ 2 (report, default rebuild)"
else
    fail "recipe-only change did not return 2"
fi

rc=0
release_build_fingerprint_matches "v1:aaa" "0.77.0" "v1:bbb" "0.77.0" "v1:r1" "v1:r1" 2>/dev/null || rc=$?
if [ "$rc" -eq 1 ]; then
    pass "content changed ⇒ 1 (rebuild), recipe tier irrelevant"
else
    fail "content change did not return 1"
fi

rc=0
release_build_fingerprint_matches "v1:aaa" "0.77.0" "v1:aaa" "0.78.0" "v1:r1" "v1:r2" 2>/dev/null || rc=$?
if [ "$rc" -eq 1 ]; then
    pass "version bump outranks the recipe tier ⇒ 1 (rebuild)"
else
    fail "version bump did not return 1"
fi

rc=0
release_build_fingerprint_matches "v1:aaa" "0.77.0" "v1:aaa" "0.77.0" 2>/dev/null || rc=$?
if [ "$rc" -eq 0 ]; then
    pass "omitted recipe args (pre-split staging) ⇒ compares content only"
else
    fail "omitted recipe args broke the content comparison"
fi

REASON="$(release_build_fingerprint_matches "v1:aaa" "0.77.0" "v1:aaa" "0.77.0" "v1:r1" "v1:r2" 2>&1 || true)"
case "$REASON" in
    *"IDENTICAL"*"recipe changed"*) pass "the recipe refusal states content was identical" ;;
    *)                              fail "unclear recipe message: '$REASON'" ;;
esac

echo ""
echo "release_build_fingerprint: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
