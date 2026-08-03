#!/usr/bin/env bash
# rebuild-mirror-history.sh: ONE-TIME repair. Rebuild the public mirror's `main`
# as a real linear commit graph of the published releases, chained
# parent-to-child, instead of 34 unrelated parentless orphans.
#
# THE PROBLEM. scripts/release-to-lucidos.sh publishes each release as a
# PARENTLESS orphan commit (release_tree_commit in scripts/lib/release_tree.sh),
# force-pushes it to lucidos/main, and pushes it BY SHA to refs/tags/<tag>. The
# mirror therefore shows a one-commit history, every tag names an object with no
# relationship to any other tag, and every release force-breaks existing clones.
#
# WHAT THIS BUILDS. The same 34 published trees, in semver order, where release
# N's parent is release N-1. v0.7 keeps zero parents; the other 33 get exactly
# one. Nothing that is published CHANGES: each new commit carries the byte-
# identical tree of the mirror commit it replaces, the identical message, and the
# identical author/committer identity and dates. The only difference between an
# old object and its replacement is one inserted `parent` line, and the script
# proves that by comparing the raw objects byte for byte (see VERIFY below).
#
# THE LEAK TRAP. Trees MUST come from the MIRROR tag SHAs. 8 of the 34 LOCAL tags
# (v0.9.6, v0.18.0 .. v0.18.5, v0.19.0) name real main-line commits carrying the
# FULL internal tree (docs/plans/, the release machinery, the private-data
# denylist) while their mirror counterparts name stripped orphans. Reading a
# tree from a local tag ref, from HEAD, or by replaying private history would
# publish exactly what the orphan contract exists to withhold, and there is no
# rollback from that. So every tag is resolved through `git ls-remote --tags`,
# every tree is `<mirror_sha>^{tree}` verbatim, the mapping is RE-RESOLVED from
# the remote before the push plan is printed, and the script prints a
# local-vs-mirror divergence audit so the 8 are visible rather than assumed.
#
# WHAT IT DOES NOT TOUCH. No local ref under refs/heads/* or refs/tags/* is
# created or moved, and the run asserts that. Same discipline as the by-SHA tag
# push at release-to-lucidos.sh:354 and for the same documented reason: the local
# v* tags must keep naming the real release commits on main, or every PREV_TAG
# guard in release.sh goes vacuous again (ADR 0029). The mirror's other branches
# (rc/*, ci/*) are never read and never written.
#
# DRY RUN BY DEFAULT. The default path is read-only against the remote: one
# `git ls-remote` and one `git fetch` (for the rollback bundle). It prints the
# planned ref updates and exits 0. Pushing requires an explicit --push plus a
# typed confirmation.
#
# Usage:
#   ./scripts/rebuild-mirror-history.sh                  # verify + print the plan
#   ./scripts/rebuild-mirror-history.sh --out-dir <dir>  # where the bundle lands
#   ./scripts/rebuild-mirror-history.sh --push           # apply it (human only)
#
# This script is a one-time repair and is registered in docs/temporary-measures.md;
# delete it once the rebuilt history is on the mirror.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

REMOTE="lucidos"
REMOTE_URL="git@github.com:lucidos-dev/lucidos.git"
BRANCH="main"
# Pinned, not derived: the whole plan was reviewed against exactly this many
# published releases. A 35th tag means a release landed since, so the mapping,
# the ordering audit and the push plan all need re-review by a human rather than
# a silently longer chain.
EXPECTED_TAGS=34
FIRST_TAG="v0.7"

step() { echo "==> $*"; }
note() { echo "    $*"; }
fail() { echo "ERROR: $*" >&2; exit 1; }

# The stripped-tree lib, for release_tree_scan / private_data_grep_tree. It is
# itself in RELEASE_TREE_EXCLUDE_PATHS, so a copy of THIS script taken from the
# public mirror has no lib to source. Say so plainly rather than dying on a
# "no such file" from `source`.
[ -f "$SCRIPT_DIR/lib/release_tree.sh" ] \
  || fail "scripts/lib/release_tree.sh is missing. It is withheld from the public mirror (RELEASE_TREE_EXCLUDE_PATHS), so this script only runs from the internal checkout."
# shellcheck source=scripts/lib/release_tree.sh
source "$SCRIPT_DIR/lib/release_tree.sh"

# ── Arguments ────────────────────────────────────────────────────────────────
DO_PUSH=0
OUT_DIR=""
while [ $# -gt 0 ]; do
  case "$1" in
    --push)    DO_PUSH=1; shift ;;
    --out-dir) [ $# -ge 2 ] || fail "--out-dir requires a directory"; OUT_DIR="$2"; shift 2 ;;
    -h|--help)
      sed -n '2,/^set -euo/p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//; $d'
      exit 0
      ;;
    *) fail "Unknown option: $1" ;;
  esac
done

STAMP="$(date -u +%Y%m%dT%H%M%SZ)"
# Outside the repo on purpose: the bundle is the rollback artifact, so it must
# not be reachable by anything that cleans, resets or rebuilds the checkout.
# Not under TMPDIR either, which macOS reaps.
[ -n "$OUT_DIR" ] || OUT_DIR="$HOME/lucidos-mirror-rebuild/$STAMP"
BACKUP_NS="refs/mirror-rebuild-backup/$STAMP"

# ── Precondition snapshots ───────────────────────────────────────────────────
# refs/tags is the one namespace that must not move AT ALL. The local v* tags
# naming the real release commits on main is the ADR 0029 invariant this whole
# script exists not to break, and nothing else on this machine creates a tag.
#
# refs/heads is deliberately NOT snapshotted wholesale. This repo hosts many
# concurrent coding-agent worktrees sharing ONE ref store, so unrelated branches
# legitimately move and appear during the minutes a run takes. Asserting global
# byte-identity there fails on other sessions' work rather than on this script's
# (observed: two sibling branches advanced mid-run). The precise invariant is
# asserted in step 8 instead, and it is the stronger one: no local ref outside
# the backup namespace may name anything this run created.
tag_refs() { git -C "$REPO_ROOT" for-each-ref --format='%(refname) %(objectname)' refs/tags; }
all_refs() { git -C "$REPO_ROOT" for-each-ref --format='%(refname) %(objectname)'; }

WORK="$(mktemp -d "${TMPDIR:-/tmp}/lucidos-rebuild-mirror.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT

tag_refs > "$WORK/tag-refs.before"
HEAD_BEFORE="$(git -C "$REPO_ROOT" rev-parse HEAD)"
MAIN_BEFORE="$(git -C "$REPO_ROOT" rev-parse -q --verify refs/heads/$BRANCH 2>/dev/null || echo '-')"

# ── 1. Preconditions ─────────────────────────────────────────────────────────
# Re-asserted rather than assumed. Each was verified by hand before this script
# was written; each is re-checked here because the run is the last thing between
# a stale fact and an irreversible push.
step "Preconditions"

git -C "$REPO_ROOT" remote get-url "$REMOTE" >/dev/null 2>&1 \
  || fail "git remote '$REMOTE' is not configured. Expected: git remote add $REMOTE $REMOTE_URL"
actual_url="$(git -C "$REPO_ROOT" remote get-url "$REMOTE")"
[ "$actual_url" = "$REMOTE_URL" ] \
  || fail "remote '$REMOTE' is $actual_url, expected $REMOTE_URL. Refusing to rebuild against an unexpected mirror."
note "remote $REMOTE = $actual_url"

step "Resolving the published tags from $REMOTE (never from a local ref)"
git -C "$REPO_ROOT" ls-remote --tags "$REMOTE" > "$WORK/ls-remote-tags" \
  || fail "Could not list tags on $REMOTE (network/auth?)."

# An annotated tag would make `<sha>` the TAG object rather than the commit, so
# the push plan and the identity comparison would describe a different object
# than the one the mirror publishes. The mirror has none today (zero peeled
# `^{}` entries); refuse if that changes.
peeled="$(grep -c '\^{}$' "$WORK/ls-remote-tags" || true)"
[ "$peeled" = "0" ] \
  || fail "$peeled annotated tag(s) on $REMOTE. This script assumes lightweight tags only; an annotated tag needs its own handling."

grep -v '\^{}$' "$WORK/ls-remote-tags" | sed 's#\trefs/tags/#\t#' > "$WORK/tag-sha"
tag_count="$(wc -l < "$WORK/tag-sha" | tr -d ' ')"
[ "$tag_count" = "$EXPECTED_TAGS" ] \
  || fail "$REMOTE has $tag_count tags, expected $EXPECTED_TAGS. A release landed since this plan was reviewed; re-review before rebuilding."
note "$tag_count lightweight tags, 0 annotated"

# ── 2. Ordering: semver, cross-checked against chronology ────────────────────
step "Ordering the releases (semver, cross-checked against committer date)"

TAGS=()
OLD_SHAS=()
OLD_TREES=()
OLD_DATES=()

while IFS= read -r tag; do
  sha="$(awk -v t="$tag" -F'\t' '$2 == t { print $1 }' "$WORK/tag-sha")"
  [ -n "$sha" ] || fail "Could not resolve $tag from the ls-remote snapshot."
  TAGS+=("$tag")
  OLD_SHAS+=("$sha")
done < <(cut -f2 "$WORK/tag-sha" | sort -V)

# The ordering loop is fed by `sort -V`. A sort that does not support -V (some
# non-GNU, non-macOS builds) writes nothing, and the array would silently come
# out empty; without this the first symptom is a bare "unbound variable" from
# set -u on the next line. Count first, so the refusal names its cause.
[ "${#TAGS[@]}" = "$EXPECTED_TAGS" ] \
  || fail "ordered ${#TAGS[@]} tags, expected $EXPECTED_TAGS. Does this sort support -V?"
[ "${TAGS[0]}" = "$FIRST_TAG" ] \
  || fail "semver order starts at ${TAGS[0]}, expected $FIRST_TAG."

# Every object must already be local. The bundle fetch below would pull them
# anyway, but checking first turns "the mirror has a tag whose object nobody
# holds" into a precondition failure instead of a mid-rebuild surprise.
i=0
while [ "$i" -lt "${#TAGS[@]}" ]; do
  sha="${OLD_SHAS[$i]}"
  git -C "$REPO_ROOT" cat-file -e "$sha^{commit}" 2>/dev/null \
    || fail "mirror commit $sha (${TAGS[$i]}) is not in the local object store."
  # THE tree read. Always `<mirror_sha>^{tree}`, never `refs/tags/<tag>^{tree}`.
  OLD_TREES+=("$(git -C "$REPO_ROOT" rev-parse --verify "$sha^{tree}")")
  OLD_DATES+=("$(git -C "$REPO_ROOT" show -s --format=%ct "$sha")")
  i=$((i + 1))
done

inversions=0
i=1
while [ "$i" -lt "${#TAGS[@]}" ]; do
  if [ "${OLD_DATES[$i]}" -lt "${OLD_DATES[$((i - 1))]}" ]; then
    echo "    INVERSION: ${TAGS[$i]} predates ${TAGS[$((i - 1))]}" >&2
    inversions=$((inversions + 1))
  fi
  i=$((i + 1))
done
[ "$inversions" = "0" ] \
  || fail "$inversions semver/chronology inversion(s). Chaining in semver order would put a later commit before an earlier one."
last=$((${#TAGS[@]} - 1))
note "semver order == chronological order, 0 inversions"
note "${TAGS[0]} ($(git -C "$REPO_ROOT" show -s --format=%as "${OLD_SHAS[0]}")) .. ${TAGS[$last]} ($(git -C "$REPO_ROOT" show -s --format=%as "${OLD_SHAS[$last]}"))"

step "Verifying $REMOTE/$BRANCH is the newest tag's commit"
# `|| fail`, not a bare assignment: a capture from a pipeline takes the
# pipeline's status and IS subject to errexit, so a network blip here would
# abort with git's stderr and no word about what the script was doing.
remote_main="$(git -C "$REPO_ROOT" ls-remote --heads "$REMOTE" "refs/heads/$BRANCH" | awk 'NR==1 {print $1}')" \
  || fail "Could not read refs/heads/$BRANCH from $REMOTE (network/auth?)."
[ -n "$remote_main" ] || fail "$REMOTE has no refs/heads/$BRANCH."
newest_sha="${OLD_SHAS[$last]}"
newest_tag="${TAGS[$last]}"
[ "$remote_main" = "$newest_sha" ] \
  || fail "$REMOTE/$BRANCH is $remote_main but $newest_tag is $newest_sha. The mirror's head is not the newest release; refusing to rebuild from an assumption that no longer holds."
note "$REMOTE/$BRANCH = $remote_main = $newest_tag"

# ── 3. Rollback bundle (before anything is built) ────────────────────────────
# The bundle is the ONLY way back if a push goes wrong, so it is captured first
# and its failure aborts the run. Fetching into a dedicated backup namespace
# (never refs/heads or refs/tags) does two jobs: it gives `git bundle create`
# named refs to work from, and it keeps the pre-rebuild objects anchored locally
# so gc cannot collect the very objects a rollback would need.
step "Capturing the rollback bundle"
mkdir -p "$OUT_DIR" || fail "Could not create $OUT_DIR"
BUNDLE="$OUT_DIR/mirror-before-rebuild.bundle"
MAP_FILE="$OUT_DIR/mirror-tag-map.tsv"

git -C "$REPO_ROOT" fetch --no-tags "$REMOTE" \
  "+refs/heads/$BRANCH:$BACKUP_NS/heads/$BRANCH" \
  "+refs/tags/*:$BACKUP_NS/tags/*" \
  || fail "Could not fetch the mirror's current refs for the rollback bundle."

# The fetched backup must agree with the ls-remote snapshot the rebuild is built
# from. Disagreement means the mirror moved between the two reads, which would
# make the bundle a rollback for a state the plan never described.
i=0
while [ "$i" -lt "${#TAGS[@]}" ]; do
  backed_up="$(git -C "$REPO_ROOT" rev-parse --verify "$BACKUP_NS/tags/${TAGS[$i]}" 2>/dev/null || true)"
  [ "$backed_up" = "${OLD_SHAS[$i]}" ] \
    || fail "backup ref for ${TAGS[$i]} is '$backed_up' but ls-remote said ${OLD_SHAS[$i]}. The mirror moved mid-run."
  i=$((i + 1))
done

BACKUP_REFS=()
while IFS= read -r r; do BACKUP_REFS+=("$r"); done \
  < <(git -C "$REPO_ROOT" for-each-ref --format='%(refname)' "$BACKUP_NS")
[ "${#BACKUP_REFS[@]}" -eq "$((EXPECTED_TAGS + 1))" ] \
  || fail "expected $((EXPECTED_TAGS + 1)) backup refs (main + $EXPECTED_TAGS tags), found ${#BACKUP_REFS[@]}."

# stdout is noise (progress + a ref list already printed above); stderr is NOT
# suppressed, because on the one artifact a rollback depends on, the reason it
# failed is the only useful thing to print.
git -C "$REPO_ROOT" bundle create "$BUNDLE" "${BACKUP_REFS[@]}" >/dev/null \
  || fail "Could not create the rollback bundle at $BUNDLE (see above)."
git -C "$REPO_ROOT" bundle verify "$BUNDLE" >/dev/null \
  || fail "The rollback bundle at $BUNDLE does not verify (see above). Refusing to continue without a usable rollback."
bundle_heads="$(git -C "$REPO_ROOT" bundle list-heads "$BUNDLE" | wc -l | tr -d ' ')"
[ "$bundle_heads" = "$((EXPECTED_TAGS + 1))" ] \
  || fail "the bundle carries $bundle_heads refs, expected $((EXPECTED_TAGS + 1))."
note "bundle:  $BUNDLE"
note "verified, $bundle_heads refs (refs/heads/$BRANCH + $EXPECTED_TAGS tags)"

# ── 4. The mapping, and the local-vs-mirror divergence audit ─────────────────
step "Snapshotting the mapping (tag -> old_sha -> old_tree)"
: > "$MAP_FILE"
printf '# captured %s from %s\n' "$STAMP" "$REMOTE" >> "$MAP_FILE"
printf '# tag\told_sha\told_tree\tlocal_tag_sha\tdiverges\n' >> "$MAP_FILE"

diverged=0
printf '    %-9s %-40s %-40s %s\n' TAG OLD_SHA OLD_TREE LOCAL
i=0
while [ "$i" -lt "${#TAGS[@]}" ]; do
  tag="${TAGS[$i]}"
  local_sha="$(git -C "$REPO_ROOT" rev-parse -q --verify "refs/tags/$tag" 2>/dev/null || echo '-')"
  if [ "$local_sha" != "-" ] && [ "$local_sha" != "${OLD_SHAS[$i]}" ]; then
    marker="DIVERGES"
    diverged=$((diverged + 1))
  else
    marker="same"
  fi
  printf '    %-9s %-40s %-40s %s\n' "$tag" "${OLD_SHAS[$i]}" "${OLD_TREES[$i]}" "$marker"
  printf '%s\t%s\t%s\t%s\t%s\n' "$tag" "${OLD_SHAS[$i]}" "${OLD_TREES[$i]}" "$local_sha" "$marker" >> "$MAP_FILE"
  i=$((i + 1))
done
note "map:     $MAP_FILE"
note "$diverged of ${#TAGS[@]} local tags name a DIFFERENT object than the mirror."
note "Those local tags carry the FULL internal tree. Every tree above was read as"
note "<mirror_sha>^{tree}; no local tag was dereferenced for a tree."

# ── 5. Rebuild ───────────────────────────────────────────────────────────────
# git commit-tree with the six identity fields read off the OLD MIRROR COMMIT
# (the same six release_tree_commit preserves), so the replacement differs from
# the original in exactly one way: it has a parent.
step "Rebuilding the chain in semver order"
NEW_SHAS=()
prev=""
i=0
while [ "$i" -lt "${#TAGS[@]}" ]; do
  tag="${TAGS[$i]}"
  old="${OLD_SHAS[$i]}"
  tree="${OLD_TREES[$i]}"

  ident="$(git -C "$REPO_ROOT" show -s --format='%an%n%ae%n%aI%n%cn%n%ce%n%cI' "$old")" \
    || fail "cannot read the identity of $old ($tag)."
  { IFS= read -r an; IFS= read -r ae; IFS= read -r ad
    IFS= read -r cn; IFS= read -r ce; IFS= read -r cd; } <<EOF
$ident
EOF
  [ -n "$an" ] && [ -n "$ae" ] && [ -n "$ad" ] && [ -n "$cn" ] && [ -n "$ce" ] && [ -n "$cd" ] \
    || fail "incomplete identity for $old ($tag)."

  # The message is taken from the RAW object (everything after the first blank
  # line) rather than from a --format placeholder, so nothing can normalise it
  # on the way through. The byte comparison below is what proves that held.
  msg="$WORK/msg.$i"
  git -C "$REPO_ROOT" cat-file commit "$old" | awk 'body { print } !body && /^$/ { body = 1 }' > "$msg"
  [ -s "$msg" ] || fail "empty commit message extracted from $old ($tag)."

  if [ -z "$prev" ]; then
    new="$(GIT_AUTHOR_NAME="$an" GIT_AUTHOR_EMAIL="$ae" GIT_AUTHOR_DATE="$ad" \
           GIT_COMMITTER_NAME="$cn" GIT_COMMITTER_EMAIL="$ce" GIT_COMMITTER_DATE="$cd" \
           git -C "$REPO_ROOT" commit-tree "$tree" -F "$msg")" \
      || fail "commit-tree failed for $tag (root of the chain)."
  else
    new="$(GIT_AUTHOR_NAME="$an" GIT_AUTHOR_EMAIL="$ae" GIT_AUTHOR_DATE="$ad" \
           GIT_COMMITTER_NAME="$cn" GIT_COMMITTER_EMAIL="$ce" GIT_COMMITTER_DATE="$cd" \
           git -C "$REPO_ROOT" commit-tree "$tree" -p "$prev" -F "$msg")" \
      || fail "commit-tree failed for $tag."
  fi
  NEW_SHAS+=("$new")
  prev="$new"
  i=$((i + 1))
done
NEW_TIP="$prev"
note "built ${#NEW_SHAS[@]} commits, tip $NEW_TIP"

# ── 6. Verify ────────────────────────────────────────────────────────────────
step "Verifying: published trees are byte-identical"
i=0
while [ "$i" -lt "${#TAGS[@]}" ]; do
  new_tree="$(git -C "$REPO_ROOT" rev-parse --verify "${NEW_SHAS[$i]}^{tree}")"
  [ "$new_tree" = "${OLD_TREES[$i]}" ] \
    || fail "${TAGS[$i]}: rebuilt tree $new_tree != published tree ${OLD_TREES[$i]}. Something other than the mirror tree was used."
  i=$((i + 1))
done
note "${#TAGS[@]}/${#TAGS[@]} trees identical to the mirror's"

# The strongest single check in the script. Reconstruct what the new object MUST
# be (the old raw object with one `parent` line inserted after the `tree` line,
# which is where git writes it) and compare byte for byte. This subsumes tree,
# message, author name/email/date and committer name/email/date in one
# comparison, and it also catches anything a per-field check would not: a
# timezone re-rendered by commit-tree, a message whose trailing whitespace was
# cleaned up, an encoding header appearing or vanishing.
step "Verifying: each new object == the old object plus one parent line"
i=0
while [ "$i" -lt "${#TAGS[@]}" ]; do
  exp="$WORK/exp.$i"
  got="$WORK/got.$i"
  if [ "$i" = "0" ]; then
    git -C "$REPO_ROOT" cat-file commit "${OLD_SHAS[$i]}" > "$exp"
  else
    git -C "$REPO_ROOT" cat-file commit "${OLD_SHAS[$i]}" \
      | awk -v p="parent ${NEW_SHAS[$((i - 1))]}" 'NR == 1 { print; print p; next } { print }' > "$exp"
  fi
  git -C "$REPO_ROOT" cat-file commit "${NEW_SHAS[$i]}" > "$got"
  cmp -s "$exp" "$got" \
    || { diff -u "$exp" "$got" >&2 || true; fail "${TAGS[$i]}: the rebuilt object differs from the published one by more than its parent line."; }
  i=$((i + 1))
done
note "${#TAGS[@]}/${#TAGS[@]} objects identical apart from the inserted parent"

step "Verifying: the chain is linear, ordered and complete"
i=0
while [ "$i" -lt "${#TAGS[@]}" ]; do
  parents="$(git -C "$REPO_ROOT" show -s --format='%P' "${NEW_SHAS[$i]}")"
  if [ "$i" = "0" ]; then
    [ -z "$parents" ] || fail "${TAGS[$i]} must be the parentless root, but has parents: $parents"
  else
    [ "$parents" = "${NEW_SHAS[$((i - 1))]}" ] \
      || fail "${TAGS[$i]}: parent is '$parents', expected ${NEW_SHAS[$((i - 1))]} (${TAGS[$((i - 1))]})."
  fi
  i=$((i + 1))
done
count="$(git -C "$REPO_ROOT" rev-list --count "$NEW_TIP")"
[ "$count" = "$EXPECTED_TAGS" ] \
  || fail "rev-list --count $NEW_TIP is $count, expected $EXPECTED_TAGS."
merges="$(git -C "$REPO_ROOT" rev-list --merges "$NEW_TIP" | wc -l | tr -d ' ')"
[ "$merges" = "0" ] \
  || fail "$merges merge commit(s) in the chain; it must be strictly linear."
# A first-parent walk from the tip must reproduce the semver order reversed. The
# per-commit parent assertion above already implies this; walking the real graph
# is the independent confirmation that it is a graph property and not just a
# property of the array the script built.
i="$last"
while IFS= read -r walked; do
  [ "$walked" = "${NEW_SHAS[$i]}" ] \
    || fail "first-parent walk position $i is $walked, expected ${NEW_SHAS[$i]} (${TAGS[$i]})."
  i=$((i - 1))
done < <(git -C "$REPO_ROOT" rev-list --first-parent "$NEW_TIP")
[ "$i" = "-1" ] || fail "the first-parent walk covered fewer commits than expected (stopped at index $i)."
note "root parentless, $last single-parent, rev-list count $count, 0 merges, first-parent walk matches"

# ── 7. Private-data guard, differentially ────────────────────────────────────
# release_tree_scan is the deterministic floor at a public push and is run on the
# tip below. It CANNOT be applied as "any hit is fatal" across the whole chain:
# 23 of the 34 already-published trees produce hits under today's denylist,
# because both the denylist and the strip list grew after those releases shipped.
# Those exact bytes have been public at those exact tag SHAs for months, so a
# zero-tolerance rule here would refuse every run while protecting nothing.
#
# What must be true is that the rebuild publishes byte-for-byte what the mirror
# already publishes and nothing more. So the guard is DIFFERENTIAL: scan the old
# mirror tree and the rebuilt commit's tree, and refuse on any difference between
# the two hit lists. A tree read from a local tag would be a full internal tree
# and would light this up immediately. Absolute counts are printed for every tag
# so the historically dirty trees stay visible rather than silently tolerated.
#
# Fail-closed in both arms, matching release_tree_scan's own contract: a scan
# that cannot RUN (the denylist will not load) refuses just as a drift does.
#
# The two sides are addressed DIFFERENTLY on purpose: the published side by the
# tree id pinned in the mapping, the rebuilt side by walking the new commit
# object. That is what makes the comparison a second, independent derivation of
# the same trees rather than the same string handed to git twice.

# pd_hits <tree-ish> <outfile>: the guard's hit list with git grep's `<tree-ish>:`
# prefix stripped. git grep echoes the tree-ish exactly AS WRITTEN, so a raw
# comparison of `<tree>` output against `<commit>^{tree}` output differs on
# every single line and says nothing about the content.
pd_hits() {
  local ish="$1" out="$2"
  private_data_grep_tree "$ish" "$REPO_ROOT" | sed "s#^${ish}:##" | sort -u > "$out"
}

step "Private-data guard over all ${#TAGS[@]} trees (differential)"
i=0
dirty=0
while [ "$i" -lt "${#TAGS[@]}" ]; do
  tag="${TAGS[$i]}"
  old_hits="$WORK/pd-old.$i"
  new_hits="$WORK/pd-new.$i"
  pd_hits "${OLD_TREES[$i]}" "$old_hits" \
    || fail "$tag: the private-data guard could not scan the published tree (denylist load or grep failure). Refusing."
  pd_hits "${NEW_SHAS[$i]}^{tree}" "$new_hits" \
    || fail "$tag: the private-data guard could not scan the rebuilt tree. Refusing."
  cmp -s "$old_hits" "$new_hits" \
    || fail "$tag: the rebuilt tree's private-data hits differ from the published tree's. The mapping is wrong and something unpublished is about to be published."
  n="$(grep -c . < "$old_hits" || true)"
  [ "$n" = "0" ] || dirty=$((dirty + 1))
  printf '    %-9s hits=%-4s (unchanged from the published tree)\n' "$tag" "$n"
  i=$((i + 1))
done
note "$dirty of ${#TAGS[@]} trees carry pre-existing hits; every one is byte-identical to what the mirror already publishes."

# The tip is the one tree a plain `git clone` checks out, and it IS expected to
# be clean, so the existing guard applies to it unmodified.
step "Private-data guard on the chain tip ($newest_tag), zero tolerance"
release_tree_scan "$REPO_ROOT" "$NEW_TIP^{tree}" \
  || fail "the private-data guard refused the tip tree. Nothing was pushed."
note "tip tree clean"

# ── 8. No local ref was created or moved by this run ─────────────────────────
step "Verifying no local ref was created or moved"

# (a) refs/tags, byte-identical. The ADR 0029 invariant: creating a local v* tag
# here is exactly the 2026-07-30 regression that left 26 of 27 tags outside
# main's history and made every PREV_TAG guard in release.sh vacuous.
tag_refs > "$WORK/tag-refs.after"
cmp -s "$WORK/tag-refs.before" "$WORK/tag-refs.after" \
  || { diff -u "$WORK/tag-refs.before" "$WORK/tag-refs.after" >&2 || true; fail "refs/tags changed during the run. The local v* tags must keep naming the real release commits on main (ADR 0029)."; }

# (b) the two refs this checkout itself sits on.
[ "$(git -C "$REPO_ROOT" rev-parse HEAD)" = "$HEAD_BEFORE" ] || fail "HEAD moved during the run."
main_after="$(git -C "$REPO_ROOT" rev-parse -q --verify "refs/heads/$BRANCH" 2>/dev/null || echo '-')"
[ "$main_after" = "$MAIN_BEFORE" ] \
  || fail "local refs/heads/$BRANCH moved during the run ($MAIN_BEFORE -> $main_after)."

# (c) The sharp one, and the reason refs/heads is not compared wholesale: NO
# local ref outside the backup namespace may name anything this run built. That
# is precisely "the rebuild polluted no local ref", and unlike a global ref diff
# it cannot be tripped by a sibling coding-agent session moving its own branch.
#
# Only the commits that are genuinely NEW objects are checked. v0.7 gains no
# parent, so its rebuilt object IS the published one bit for bit, and the local
# v0.7 tag legitimately names it; including it would flag a pre-existing ref.
all_refs > "$WORK/all-refs.after"
# Filter on the REFNAME prefix (field 1), not a substring of the whole line: the
# backup refs sit at the start of their line, so a pattern carrying a leading
# space would never match one and the exclusion would silently do nothing.
awk -v ns="$BACKUP_NS/" 'substr($1, 1, length(ns)) != ns { print $2 }' \
  "$WORK/all-refs.after" | sort -u > "$WORK/local-ref-objects"
i=0
leaked=""
while [ "$i" -lt "${#TAGS[@]}" ]; do
  if [ "${NEW_SHAS[$i]}" != "${OLD_SHAS[$i]}" ] \
     && grep -qx -- "${NEW_SHAS[$i]}" "$WORK/local-ref-objects"; then
    leaked="$leaked ${TAGS[$i]}"
  fi
  i=$((i + 1))
done
[ -z "$leaked" ] \
  || fail "a local ref outside $BACKUP_NS names a rebuilt commit ($leaked). This run must create no local ref."
note "refs/tags byte-identical; HEAD and local $BRANCH unmoved; no local ref names a rebuilt commit"

# ── 9. Re-resolve the mapping against the live remote ────────────────────────
# Everything above was built from one ls-remote snapshot taken at the start. If
# the mirror moved since, the plan below would describe a state that no longer
# exists, and on the --push path it would clobber whatever arrived.
step "Re-resolving the mapping against $REMOTE (guard against a mirror that moved mid-run)"
git -C "$REPO_ROOT" ls-remote --tags "$REMOTE" | grep -v '\^{}$' | sed 's#\trefs/tags/#\t#' > "$WORK/tag-sha.recheck" \
  || fail "Could not re-list tags on $REMOTE."
sort "$WORK/tag-sha" > "$WORK/tag-sha.sorted"
sort "$WORK/tag-sha.recheck" > "$WORK/tag-sha.recheck.sorted"
cmp -s "$WORK/tag-sha.sorted" "$WORK/tag-sha.recheck.sorted" \
  || fail "the mirror's tags changed during this run. Re-run from a fresh snapshot."
remote_main_now="$(git -C "$REPO_ROOT" ls-remote --heads "$REMOTE" "refs/heads/$BRANCH" | awk 'NR==1 {print $1}')" \
  || fail "Could not re-read refs/heads/$BRANCH from $REMOTE (network/auth?)."
[ "$remote_main_now" = "$remote_main" ] \
  || fail "$REMOTE/$BRANCH moved during this run ($remote_main -> $remote_main_now). Re-run from a fresh snapshot."
note "mapping and $BRANCH unchanged"

# ── 10. The push, composed as ONE atomic compare-and-swap ────────────────────
# Two properties, both load-bearing, and neither is available from a loop of
# bare `git push --force`:
#
#   --atomic       All 35 refs move together or none does. Pushed one at a time,
#                  a failure on tag 12 leaves the mirror with a rebuilt `main`,
#                  11 rebuilt tags and 23 orphan tags, which is a state no
#                  release ever produced and which needs a manual rollback to
#                  leave. There is no partial success worth keeping here.
#
#   --force-with-lease=<ref>:<old>
#                  Every ref update is conditional on the ref still holding the
#                  exact SHA this run verified. Step 9 re-resolved the mapping,
#                  but a plain --force turns that into a check-then-act with the
#                  confirmation prompt sitting in the gap: a release landing in
#                  that window would be silently overwritten. The lease moves
#                  the precondition to the server, at update time, where it
#                  cannot be raced. The expected values are named explicitly
#                  (rather than relying on remote-tracking refs, which this
#                  repo has none of for the mirror's tags).
#
# Composed once and used for both the printed plan and the actual push, so the
# command a human reads is the command that runs.
PUSH_OPTS=(--atomic "--force-with-lease=refs/heads/$BRANCH:$remote_main")
PUSH_REFSPECS=("$NEW_TIP:refs/heads/$BRANCH")
i=0
while [ "$i" -lt "${#TAGS[@]}" ]; do
  PUSH_OPTS+=("--force-with-lease=refs/tags/${TAGS[$i]}:${OLD_SHAS[$i]}")
  PUSH_REFSPECS+=("${NEW_SHAS[$i]}:refs/tags/${TAGS[$i]}")
  i=$((i + 1))
done

echo
echo "================================================================================"
echo " PLANNED REF UPDATES  (nothing above touched the mirror)"
echo "================================================================================"
echo
echo "  ONE atomic push, each ref leased to the SHA verified above:"
echo
echo "  git push \\"
for o in "${PUSH_OPTS[@]}"; do printf '    %s \\\n' "$o"; done
printf '    %s \\\n' "$REMOTE"
i=0
while [ "$i" -lt "${#PUSH_REFSPECS[@]}" ]; do
  if [ "$i" = "$((${#PUSH_REFSPECS[@]} - 1))" ]; then
    printf '    %s\n' "${PUSH_REFSPECS[$i]}"
  else
    printf '    %s \\\n' "${PUSH_REFSPECS[$i]}"
  fi
  i=$((i + 1))
done
echo
echo "  Left alone: every other ref on the mirror (rc/*, ci/*), and every local ref."
echo
echo "  Rollback:  git push --force $REMOTE $remote_main:refs/heads/$BRANCH"
echo "             plus each tag from $MAP_FILE, or from the bundle:"
echo "             $BUNDLE"
echo

if [ "$DO_PUSH" != "1" ]; then
  echo "  DRY RUN. Nothing was pushed. Re-run with --push to apply."
  echo
  exit 0
fi

# ── 11. Push (explicit, confirmed, human-only) ───────────────────────────────
echo "  --push given. This moves $BRANCH and all $EXPECTED_TAGS tags on the PUBLIC mirror."
echo "  A ref that moved since the verification above will REFUSE the whole push."
printf '  Type the word REBUILD to proceed: '
read -r confirm
[ "$confirm" = "REBUILD" ] || fail "not confirmed; nothing was pushed."

step "Pushing the rebuilt chain to $REMOTE (atomic, leased)"
git -C "$REPO_ROOT" push "${PUSH_OPTS[@]}" "$REMOTE" "${PUSH_REFSPECS[@]}" \
  || fail "the atomic push was refused, so NOTHING moved on the mirror. Either a ref changed since this run verified it (re-run from a fresh snapshot), or the server rejected --atomic. The mirror is exactly as it was; $BUNDLE is still your rollback."

echo
echo "Done. The mirror's $BRANCH is now a linear history of $EXPECTED_TAGS releases."
echo "Rollback bundle kept at $BUNDLE"
