#!/bin/bash
# Tests that the repo has exactly ONE source of truth for the release version.
#
# The repo-root RELEASE file IS that source. Everything else must DERIVE from
# it — at build time (build.rs reads it), at release time (release.sh rewrites
# install.sh's baked constant), or at publish time (the site publisher pins its
# download links from it). A second hand-maintained copy is drift waiting to
# happen, and each one has already bitten:
#
#   • install.sh's baked LUCIDOS_DEFAULT_VERSION sat at 0.14.0 while RELEASE
#     moved to 0.15.0. That constant is what a piped `curl … | sh` resolves, and
#     0.14.0 predates the headless tarballs — so the advertised one-liner 404'd.
#   • CONTRIBUTING.md announced "currently on the 0.9.x line" while main was on
#     0.16.0 — nobody re-reads prose at release time.
#
# So this suite scans the tracked tree for version literals that nothing keeps
# in sync, and pins the two derivation mechanisms that already exist. A new
# hardcoded version fails here instead of shipping.
#
# Run: ./scripts/lib/version_sources_test.sh
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
cd "$PROJECT_DIR" || exit 1

PASS=0
FAIL=0
pass() { echo "  ok:   $*"; PASS=$((PASS + 1)); }
fail() { echo "  FAIL: $*"; FAIL=$((FAIL + 1)); }

RELEASE_VERSION="$(tr -d '[:space:]' < RELEASE 2>/dev/null)"

echo "test: RELEASE is the single source and is well-formed"
if [[ "$RELEASE_VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  pass "RELEASE = $RELEASE_VERSION"
else
  fail "RELEASE holds '$RELEASE_VERSION', which is not an x.y.z version"
fi

# ── The one sanctioned duplicate, and the machinery that keeps it honest ─────
# install.sh's baked constant cannot derive at runtime: a piped install has no
# checkout to read RELEASE from. So it is written at RELEASE-bump time instead,
# and asserted equal here. (install_test.sh asserts the same equality from the
# installer's side; this suite additionally pins the WRITER, so deleting the
# substitution can't leave the equality passing only until the next bump.)
echo "test: install.sh's baked default is kept in lockstep by release.sh"
baked="$(sed -n 's/^LUCIDOS_DEFAULT_VERSION="\([^"]*\)".*/\1/p' install.sh | head -1)"
if [ "$baked" = "$RELEASE_VERSION" ]; then
  pass "baked LUCIDOS_DEFAULT_VERSION = RELEASE = $baked"
else
  fail "install.sh LUCIDOS_DEFAULT_VERSION='$baked' != RELEASE '$RELEASE_VERSION'"
fi
if grep -q 's/\^LUCIDOS_DEFAULT_VERSION=' scripts/release.sh; then
  pass "release.sh rewrites the baked constant when it bumps RELEASE"
else
  fail "release.sh no longer rewrites LUCIDOS_DEFAULT_VERSION — the next bump silently strands it"
fi

# ── The scan: no OTHER file may carry a release-version literal ──────────────
# Deliberately shape-based, not a list of known offenders: the point is to catch
# the file nobody thought of. Scoped to the tracked tree, minus the places where
# a version literal is legitimate:
#   CHANGELOG.md / docs/plans / docs/adr / docs/audits: historical records. They
#     SHOULD name the version they describe, and rewriting history is the actual
#     bug. docs/audits joined the list on 2026-08-02, when the first audit landed
#     and immediately reddened this gate: its findings are per-release evidence
#     (`CFBundleShortVersionString = 0.19.0`, a shasum of a named tarball), which
#     is the same class as a CHANGELOG entry and must not move when RELEASE does.
#   *.lock, package-lock.json, node_modules — dependency versions, not ours.
#   .github/dependabot.yml: same class, it exists to reason about THIRD-PARTY
#     versions (the glib/gtk/tao/tauri resolution chain), so every literal in it
#     is someone else's. Collisions with our own version are inevitable and
#     recurring rather than a one-off: that file already names 0.18.2, 0.18.5,
#     0.20.0, 0.35.3 and 2.11.4, and it blocked the 0.18.2 release when the
#     release number happened to equal the resolved gtk version in a comment.
#     Rewording the comment would only defer the same false positive to 0.20.0.
#   RELEASE, install.sh — the source and its sanctioned, machine-written copy.
# 0.0.0 and 0.1.0 are excluded: they're the unpublished crate/package version
# and build.rs's fallback, neither of which tracks the release.
echo "test: no unmanaged release-version literal anywhere else in the tree"
# NOTE: `-w`, not `\b`. `git grep -E` is POSIX ERE, where `\b` is NOT a word
# boundary — it silently matches nothing, so the first cut of this scan passed
# on every planted literal. Use the `-w` flag for word boundaries.
#
# NOTE: the leading '.' pathspec is REQUIRED. A `git grep -- ':(exclude)x'`
# whose pathspecs are ALL exclusions matches NOTHING — git needs at least one
# positive pathspec to define the search set. Without it these scans quietly
# searched an empty tree and passed on a planted literal. Never drop the '.'.
#
# NOTE: no `mapfile` / `readarray` here — macOS ships bash 3.2, where they do
# not exist. The first cut of this suite used them and the scan tests PRINTED
# THEIR HEADING AND SILENTLY DID NOTHING, which is precisely the fail-open
# behaviour a guard must never have. A temp file + `wc -l` works everywhere.
HITS_FILE="$(mktemp -t version-sources-hits)"
STALE_FILE="$(mktemp -t version-sources-stale)"
trap 'rm -f "$HITS_FILE" "$HITS_FILE".all "$HITS_FILE".raw "$STALE_FILE"' EXIT

# `git grep` exits 1 on "no matches" — the CLEAN case — so its status is
# captured in a condition context rather than left to trip errexit in a caller
# that runs this suite under `set -e`. (Same hazard as the private-data guard;
# see scripts/lib/private_data_patterns.sh.)
if git grep -nIwE '[0-9]+\.[0-9]+\.[0-9]+' -- \
    '.' \
    ':(exclude)CHANGELOG.md' \
    ':(exclude)RELEASE' \
    ':(exclude)install.sh' \
    ':(exclude)docs/plans' \
    ':(exclude)docs/adr' \
    ':(exclude)docs/audits' \
    ':(exclude)*.lock' \
    ':(exclude)*package-lock.json' \
    ':(exclude).github/dependabot.yml' \
    ':(exclude)**/node_modules/**' \
    ':(exclude)scripts/lib/version_sources_test.sh' \
    > "$HITS_FILE".all 2>/dev/null; then
  :
elif [ ! -f "$HITS_FILE".all ]; then
  fail "the version scan could not run — treating that as a failure, not a clean tree"
fi
grep -wE "$RELEASE_VERSION" "$HITS_FILE".all > "$HITS_FILE".raw 2>/dev/null || true
# A dependency RANGE in a package manifest is someone else's version, the same
# class as .github/dependabot.yml above. "esbuild": "^0.25.0" collided with the
# 0.25.0 release, and nothing about that range tracks ours. Only the caret and
# tilde forms are dropped, and only in a package.json, so a BARE literal is
# still caught: a manifest's own "version" field, an exact dependency pin, and
# the number written anywhere else in the tree all still fail this test.
grep -vE '^[^:]*package\.json:[0-9]+:.*"[~^]'"$RELEASE_VERSION"'"' \
    "$HITS_FILE".raw > "$HITS_FILE" 2>/dev/null || true
hit_count="$(wc -l < "$HITS_FILE" | tr -d '[:space:]')"
if [ "$hit_count" -eq 0 ]; then
  pass "no file hardcodes the current release version ($RELEASE_VERSION)"
else
  fail "the current release version is hardcoded in $hit_count place(s) that nothing keeps in sync:"
  sed 's/^/          /' "$HITS_FILE"
  echo "        Derive it instead (read RELEASE at build/release/publish time),"
  echo "        or add a deliberate exclusion above with the reason."
fi

# A version literal that is merely STALE is the same bug caught one release
# later, so also flag prose that announces WHICH LINE the project is on. Scoped
# to that claim-shape ("is/currently on … 0.x") rather than any version mention:
# a doc that NARRATES a past release ("0.14.0 predates headless tarballs", a
# sample updater manifest pinned to 0.10.0) is a historical record and is
# correct precisely because it does not move. What rots is a doc asserting the
# CURRENT version, because nobody re-reads prose at release time — that is how
# CONTRIBUTING.md came to announce the 0.9.x line while main was on 0.16.0.
#
# "currently the" / "currently at" are in the alternation because CONTRIBUTING.md
# was not the only file making that claim: SECURITY.md said "pre-1.0 (currently
# the 0.9.x line)" and sailed through this scan until the 2026-07-30 docs audit
# read it by hand, because the first cut only knew the preposition "on". Match
# the ADVERB plus any of its prepositions, not one fixed phrase. Bare "currently"
# is deliberately NOT accepted: it would fire on ordinary prose that happens to
# mention a version-shaped token ("gpt-5.5") within 40 characters.
echo "test: no prose announces which release line the project is currently on"
git grep -nIiE "(currently (on|at|the)|is now on|we are on|latest release is|current version is)[^.]{0,40}[0-9]+\.[0-9]+\.?[0-9x]*" \
  -- '*.md' \
  ':(exclude)CHANGELOG.md' ':(exclude)docs/plans' ':(exclude)docs/adr' \
  ':(exclude)docs/audits' \
  ':(exclude)scripts/lib/version_sources_test.sh' \
  > "$STALE_FILE" 2>/dev/null || true
stale_count="$(wc -l < "$STALE_FILE" | tr -d '[:space:]')"
if [ "$stale_count" -eq 0 ]; then
  pass "no doc claims to know the current release line"
else
  fail "prose announces the current release line in $stale_count place(s) — it dates on the next release:"
  cut -c1-160 "$STALE_FILE" | sed 's/^/          /'
  echo "        Point at the newest tag / the RELEASE file instead."
fi

echo ""
if [ "$FAIL" -gt 0 ]; then
  echo "  ($PASS passed, $FAIL failed)"
  exit 1
fi
echo "  ($PASS passed, 0 failed)"
