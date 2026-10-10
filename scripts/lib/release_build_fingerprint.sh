#!/usr/bin/env bash
# release_build_fingerprint.sh — the compiled-input fingerprint.
#
# WHY THIS EXISTS (the 2026-07-28 incident)
# ------------------------------------------------------------------------------
# A single release made SEVEN Apple notarization submissions in one day (2026-07-28).
# Three of them were byte-identical compiled input: `git rev-parse HEAD:crates`
# and `HEAD:Cargo.lock` were the same for the 14:46, 16:06 and 16:16 builds. The
# commits that forced those rebuilds were a CONTRIBUTING.md wording change, a
# release-version test, and an install.sh fix — not one byte the Rust or TS
# compiler ever sees. Each redundant rebuild cost a fresh trip through Apple's
# notary queue, and on a day when that queue was wedged, each one deepened the
# very backlog we were stuck in.
#
# A release re-fold onto new main is CORRECT and must stay the default (see the
# release-process knowhow: never ship a stale candidate). What was wrong is that
# a re-fold unconditionally implied "rebuild + resubmit". This library lets the
# re-fold ask a sharper question: does the new tree actually produce different
# BYTES? When it doesn't, the already-built (and possibly already-in-flight or
# already-notarized) DMG is still exactly right, and the release commit can move
# forward without touching Apple.
#
# WHAT IS FINGERPRINTED
# ------------------------------------------------------------------------------
# Content trees, NOT the commit sha. `git rev-parse HEAD:<path>` yields a stable
# tree/blob hash for a path, so two different commits with identical content for
# a path yield the identical hash — which is precisely the "docs-only commit"
# case we need to detect. The tracked set is everything that ends up inside the
# .app bundle, per RESOURCE_NAMES in build-dmg.sh:
#
#   crates            — the engine, gateway, CLI, and the Tauri app (incl. the
#                       frontend source at crates/lucidos-app/src and
#                       tauri.conf.json, both under this tree)
#   Cargo.lock        — the exact dependency versions compiled in
#   Cargo.toml        — workspace members, profiles, feature defaults
#   packages          — the JS SDK, staged into Resources/sdk
#   package.json      — npm workspace definition used by the frontend build
#   package-lock.json — the exact JS dependency versions built in
#   system-knowhow    — copied verbatim into Resources/system-knowhow, so a
#                       change here genuinely changes the shipped bundle
#
# DELIBERATELY EXCLUDED, and why each is safe:
#   RELEASE, CHANGELOG.md — they change on EVERY release commit by construction.
#     Including them would make the fingerprint differ on every re-fold and the
#     gate would never fire. THE VERSION TRAP: the version string IS compiled
#     into the app (Tauri stamps it from RELEASE via --config, and the DMG is
#     named Lucidos_<version>_aarch64.dmg). So excluding RELEASE is only sound
#     while the target version is unchanged. That is NOT left as an unstated
#     assumption — release_build_fingerprint_matches() takes the version as an
#     explicit argument and refuses a match when it differs. Within one release
#     the version is constant across re-folds (every build in the 2026-07-28
#     incident carried the same version), so the gate is sound there and
#     correctly declines across a bump.
#   install.sh, CONTRIBUTING.md, docs/, .github/, README, tests/ — not compiled,
#     not bundled. These are exactly the files whose changes triggered the three
#     wasted submissions.
#
# THE SECOND TIER: the build recipe
# ------------------------------------------------------------------------------
# scripts/build-dmg.sh and scripts/lib/stage_runtime.sh do not get compiled into
# anything, but they DECIDE what lands in Resources and how it is signed — so a
# real change to either can change the shipped bundle without touching a line of
# source. They cannot simply be ignored.
#
# They also cannot go in the content tier. Verified against the real incident:
# commit e4a32b901 changed exactly one COMMENT in build-dmg.sh (it renamed the
# incident it referred to) and nothing else. Folding that into the
# content fingerprint forces a full rebuild + a fresh notary submission for a
# comment — re-creating the precise waste this gate exists to stop.
#
# So the recipe is a SEPARATE fingerprint with a SEPARATE verdict, and the
# default on a recipe change is still to rebuild (status quo, safe). The
# difference is that the operator is told exactly which recipe file moved and can
# skip deliberately when the change is provably cosmetic. Silence is never the
# answer in either direction: a recipe change is always reported.

# ── Tier 1: content compiled or copied into the shipped bundle. ──────────────
# The tracked paths, in a fixed order (the fingerprint is order-dependent by
# construction, and a stable order keeps it reproducible across runs).
RELEASE_FINGERPRINT_PATHS=(
    crates
    Cargo.lock
    Cargo.toml
    packages
    package.json
    package-lock.json
    system-knowhow
)

# ── Tier 2: the build recipe — decides what gets bundled and how it is signed,
# but is not itself shipped. Reported separately (see the header block).
RELEASE_FINGERPRINT_RECIPE_PATHS=(
    scripts/build-dmg.sh
    scripts/lib/stage_runtime.sh
)

# release_build_fingerprint_compute <repo-root> [<commit-ish>] — print the
# fingerprint of the compiled inputs at <commit-ish> (default HEAD).
#
# Output shape: "v1:<64 hex>" — the version prefix so a future change to the
# tracked path set can invalidate old fingerprints deliberately rather than
# silently comparing across two different definitions.
#
# A path that does not exist at that commit contributes the literal "-" rather
# than failing: the set must be able to name a path that is legitimately absent
# in an older tree without making the whole fingerprint unavailable.
release_build_fingerprint_compute() {
    local root="${1:-.}" rev="${2:-HEAD}" path hash lines=""

    git -C "$root" rev-parse --verify --quiet "$rev^{commit}" >/dev/null \
        || { echo "ERROR: not a commit in $root: $rev" >&2; return 1; }

    for path in "${RELEASE_FINGERPRINT_PATHS[@]}"; do
        hash="$(git -C "$root" rev-parse --verify --quiet "$rev:$path" 2>/dev/null || true)"
        [ -n "$hash" ] || hash="-"
        lines+="$path $hash"$'\n'
    done

    printf 'v1:%s' "$(printf '%s' "$lines" | python3 -c '
import hashlib, sys
sys.stdout.write(hashlib.sha256(sys.stdin.buffer.read()).hexdigest())
')"
}

# release_build_fingerprint_explain <repo-root> <rev-a> <rev-b> — print the
# tracked paths whose content differs between two commits, one per line, as
# "<path>  <hash-a> -> <hash-b>". Empty output means the compiled inputs are
# identical. This is the operator-facing "why did it rebuild?" answer, so it is
# printed whenever the gate declines to skip.
release_build_fingerprint_explain() {
    local root="$1" a="$2" b="$3" path ha hb
    for path in "${RELEASE_FINGERPRINT_PATHS[@]}"; do
        ha="$(git -C "$root" rev-parse --verify --quiet "$a:$path" 2>/dev/null || true)"
        hb="$(git -C "$root" rev-parse --verify --quiet "$b:$path" 2>/dev/null || true)"
        [ -n "$ha" ] || ha="-"
        [ -n "$hb" ] || hb="-"
        [ "$ha" = "$hb" ] && continue
        printf '%s  %s -> %s\n' "$path" "${ha:0:12}" "${hb:0:12}"
    done
}

# release_build_recipe_fingerprint_compute <repo-root> [<commit-ish>] — the
# tier-2 fingerprint over the build-recipe scripts. Same shape as the content
# fingerprint (v1:<64 hex>) so the two are stored and compared identically.
release_build_recipe_fingerprint_compute() {
    local root="${1:-.}" rev="${2:-HEAD}" path hash lines=""

    git -C "$root" rev-parse --verify --quiet "$rev^{commit}" >/dev/null \
        || { echo "ERROR: not a commit in $root: $rev" >&2; return 1; }

    for path in "${RELEASE_FINGERPRINT_RECIPE_PATHS[@]}"; do
        hash="$(git -C "$root" rev-parse --verify --quiet "$rev:$path" 2>/dev/null || true)"
        [ -n "$hash" ] || hash="-"
        lines+="$path $hash"$'\n'
    done

    printf 'v1:%s' "$(printf '%s' "$lines" | python3 -c '
import hashlib, sys
sys.stdout.write(hashlib.sha256(sys.stdin.buffer.read()).hexdigest())
')"
}

# release_build_recipe_explain <repo-root> <rev-a> <rev-b> — the tier-2 counterpart
# of release_build_fingerprint_explain.
release_build_recipe_explain() {
    local root="$1" a="$2" b="$3" path ha hb
    for path in "${RELEASE_FINGERPRINT_RECIPE_PATHS[@]}"; do
        ha="$(git -C "$root" rev-parse --verify --quiet "$a:$path" 2>/dev/null || true)"
        hb="$(git -C "$root" rev-parse --verify --quiet "$b:$path" 2>/dev/null || true)"
        [ -n "$ha" ] || ha="-"
        [ -n "$hb" ] || hb="-"
        [ "$ha" = "$hb" ] && continue
        printf '%s  %s -> %s\n' "$path" "${ha:0:12}" "${hb:0:12}"
    done
}

# release_build_fingerprint_matches <staged-fp> <staged-version> <candidate-fp>
#                                   <candidate-version>
#                                   [<staged-recipe-fp> <candidate-recipe-fp>]
#
# The gate. TRI-STATE, because the two tiers carry different risk:
#
#   0 — full match. Compiled inputs identical AND target version identical AND
#       (when supplied) the build recipe identical. Safe to skip the rebuild and
#       keep the existing DMG / in-flight submission.
#   2 — content match, RECIPE CHANGED. The shipped source is byte-identical but
#       build-dmg.sh / stage_runtime.sh moved, so the bundle *could* differ. The
#       caller must default to rebuilding and may only skip on an explicit
#       operator override. The changed files are named on stderr.
#   1 — rebuild. Compiled inputs differ, the version differs, or a fingerprint is
#       missing (which must NEVER read as "unchanged").
#
# The version argument is what makes excluding RELEASE from the fingerprint sound
# — see the version trap in the header. It is an explicit, asserted precondition,
# not an assumption.
#
# The recipe arguments are optional so a staged artifact written before the
# two-tier split still compares cleanly on its content tier.
release_build_fingerprint_matches() {
    local staged_fp="${1:-}" staged_ver="${2:-}" cand_fp="${3:-}" cand_ver="${4:-}"
    local staged_recipe="${5:-}" cand_recipe="${6:-}"

    if [ -z "$staged_fp" ]; then
        echo "build-fingerprint: the staged artifact records no fingerprint (built before this gate existed) — rebuilding." >&2
        return 1
    fi
    if [ -z "$cand_fp" ]; then
        echo "build-fingerprint: could not compute a fingerprint for the candidate tree — rebuilding." >&2
        return 1
    fi
    if [ -z "$staged_ver" ] || [ -z "$cand_ver" ]; then
        echo "build-fingerprint: missing version on one side (staged='$staged_ver' candidate='$cand_ver') — rebuilding." >&2
        return 1
    fi
    if [ "$staged_ver" != "$cand_ver" ]; then
        echo "build-fingerprint: target version changed ($staged_ver -> $cand_ver)." >&2
        echo "                   The version string is COMPILED INTO the app and stamped into the" >&2
        echo "                   DMG name, so the staged DMG is stale regardless of source — rebuilding." >&2
        return 1
    fi
    if [ "$staged_fp" != "$cand_fp" ]; then
        echo "build-fingerprint: compiled inputs changed." >&2
        echo "                   staged:    $staged_fp" >&2
        echo "                   candidate: $cand_fp" >&2
        return 1
    fi

    # Content is identical. Now the recipe tier — reported, never silently
    # ignored, but distinguished from a real source change so the operator can
    # tell a cosmetic bundler edit from a behavioural one.
    if [ -n "$staged_recipe" ] && [ -n "$cand_recipe" ] \
       && [ "$staged_recipe" != "$cand_recipe" ]; then
        echo "build-fingerprint: compiled inputs are IDENTICAL, but the build recipe changed." >&2
        echo "                   staged recipe:    $staged_recipe" >&2
        echo "                   candidate recipe: $cand_recipe" >&2
        return 2
    fi
    return 0
}
