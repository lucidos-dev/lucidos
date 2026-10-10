#!/usr/bin/env bash
#
# harden-scope.sh: decide how much a re-run of `/harden` must review, given
# what the last hardening already covered (ADR 0295, ADR 0315).
#
#   ./scripts/harden-scope.sh "$(lucidos hardened sha 2>/dev/null)" [<base>]
#
# The first line is the answer, and the lines after it name what to review:
#
#   MERGE_ONLY       every commit after the hardened SHA, apart from commits
#                    already on <base> (default main), is a merge of <base>
#   INCREMENTAL      the branch has commits of its own since then as well
#
#   commit <sha>     a branch commit since the hardening, oldest first
#   merge <sha>      a merge since the hardening, oldest first
#   overlap <path>   a file the branch changed that a merge also brought in
#   codex-base <sha> INCREMENTAL with no merge: the base for the Codex review
#
# Otherwise it prints one line, FULL <reason>. Anything git cannot answer is
# FULL too: a narrow answer skips unreviewed code, a wrong FULL only costs time.
#
# Exit status: 0 for every answer.

set -uo pipefail

sha=${1:-}
base=${2:-main}

full() {
    echo "FULL $*"
    exit 0
}

[ -n "$sha" ] || full "no harden marker"
git rev-parse --verify --quiet "$sha^{commit}" >/dev/null || full "hardened commit not found: $sha"
git rev-parse --verify --quiet "$base^{commit}" >/dev/null || full "base not found: $base"
git merge-base --is-ancestor "$sha" HEAD || full "hardened commit is not in HEAD's history"
git show --remerge-diff --format= -s HEAD >/dev/null 2>&1 || full "this git has no --remerge-diff"

merges=$(git rev-list --merges --reverse "$sha..HEAD" "^$base") || full "git rev-list failed"
for m in $merges; do
    # Every parent after the first must be <base> history, or the merge
    # brought in commits nobody reviewed that this script cannot list.
    side=$(git rev-parse "$m^@" | tail -n +2 | git rev-list --stdin "^$m^1" "^$base") ||
        full "git rev-list failed for $m"
    [ -z "$side" ] || full "a merge brought in commits the base lacks"
done
own=$(git rev-list --no-merges --reverse "$sha..HEAD" "^$base") || full "git rev-list failed"
[ -n "$own$merges" ] || full "no commit since the last hardening"

branch_files=$(git diff --name-only "$base...HEAD" | sort -u) || full "git diff failed"
brought=""
for m in $merges; do
    brought="$brought$(git diff --name-only "$m^1" "$m")
" || full "git diff failed for $m"
done

if [ -n "$own" ]; then echo INCREMENTAL; else echo MERGE_ONLY; fi
for c in $own; do
    echo "commit $c"
done
for m in $merges; do
    echo "merge $m"
done
comm -12 <(printf '%s\n' "$branch_files") <(printf '%s' "$brought" | sort -u) |
    sed -n 's/^\(..*\)$/overlap \1/p'
# With a merge in range, <sha>...HEAD holds all of main's new code as well.
[ -n "$own" ] && [ -z "$merges" ] && echo "codex-base $sha"
exit 0
