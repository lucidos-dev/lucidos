#!/usr/bin/env bash
# Push the current tree as a single squashed commit to the `lucidos` git remote.
#
# Lucidos is the public, user-facing release of CognOS. Internal history,
# branches, and per-step commits stay on `origin`; the public mirror only ever
# sees one orphan commit per release, tagged `v<RELEASE>`.
#
# Usage:
#   ./scripts/release-to-lucidos.sh                       # bare release
#   ./scripts/release-to-lucidos.sh "summary of changes"  # with summary
#
# Prerequisites:
#   - Working tree must be clean
#   - A `lucidos` git remote must exist:
#       git remote add lucidos git@github.com:lucidos-dev/lucidos.git

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
RELEASE_FILE="$REPO_ROOT/RELEASE"
REMOTE="lucidos"
BRANCH="main"

step() { echo "==> $*"; }
fail() { echo "ERROR: $*" >&2; exit 1; }

step "Reading $RELEASE_FILE"
[[ -f "$RELEASE_FILE" ]] || fail "RELEASE file not found at $RELEASE_FILE"
release="$(tr -d '[:space:]' < "$RELEASE_FILE")"
[[ -n "$release" ]] || fail "RELEASE file is empty"
echo "    release = $release"

step "Verifying '$REMOTE' remote exists"
if ! git -C "$REPO_ROOT" remote get-url "$REMOTE" >/dev/null 2>&1; then
  cat <<EOF >&2
ERROR: git remote '$REMOTE' is not configured.

To configure it:
  git remote add $REMOTE git@github.com:lucidos-dev/lucidos.git

Then re-run this script.
EOF
  exit 1
fi
echo "    $REMOTE -> $(git -C "$REPO_ROOT" remote get-url "$REMOTE")"

step "Verifying working tree is clean"
if ! git -C "$REPO_ROOT" diff --quiet || ! git -C "$REPO_ROOT" diff --cached --quiet; then
  fail "Working tree has uncommitted changes. Commit or stash before releasing."
fi
if [[ -n "$(git -C "$REPO_ROOT" status --porcelain)" ]]; then
  fail "Working tree has untracked changes. Clean before releasing."
fi

summary="${1:-}"
if [[ -n "$summary" ]]; then
  message="Release $release - $summary"
else
  message="Release $release"
fi
tag="v$release"
temp_branch="release/lucidos-$release-$(date +%s)"

step "Capturing current HEAD tree (excluding internal-only paths)"
# Build a release tree in a temporary index so we can drop paths that should
# stay internal — planning notes, internal-only docs — without touching the
# user's working tree or real index.
EXCLUDE_PATHS=(
  "docs/plans"
)
RELEASE_INDEX="$(mktemp -t lucidos-release-index)"
trap 'rm -f "$RELEASE_INDEX"' EXIT
GIT_INDEX_FILE="$RELEASE_INDEX" git -C "$REPO_ROOT" read-tree HEAD
for path in "${EXCLUDE_PATHS[@]}"; do
  GIT_INDEX_FILE="$RELEASE_INDEX" git -C "$REPO_ROOT" rm --cached -rq -- "$path" 2>/dev/null || true
done
tree="$(GIT_INDEX_FILE="$RELEASE_INDEX" git -C "$REPO_ROOT" write-tree)"
echo "    tree     = $tree"
echo "    excluded = ${EXCLUDE_PATHS[*]}"

step "Creating orphan commit '$message'"
commit="$(git -C "$REPO_ROOT" commit-tree "$tree" -m "$message")"
echo "    commit = $commit"

step "Creating temp branch $temp_branch"
git -C "$REPO_ROOT" branch "$temp_branch" "$commit"

step "Force-pushing to $REMOTE/$BRANCH"
git -C "$REPO_ROOT" push --force "$REMOTE" "$temp_branch:refs/heads/$BRANCH"

step "Creating and pushing tag $tag"
# Replace any existing local tag of the same name so re-runs are idempotent.
git -C "$REPO_ROOT" tag -f "$tag" "$commit"
git -C "$REPO_ROOT" push --force "$REMOTE" "refs/tags/$tag"

step "Deleting temp branch $temp_branch"
git -C "$REPO_ROOT" branch -D "$temp_branch"

cat <<EOF

Release pushed.

  Commit:  $commit
  Tag:     $tag
  Remote:  $REMOTE/$BRANCH

  https://github.com/lucidos-dev/lucidos/releases/tag/$tag
EOF
