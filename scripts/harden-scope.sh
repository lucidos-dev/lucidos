#!/usr/bin/env bash
#
# harden-scope.sh: decide how much a re-run of `/harden` must review, given
# what the last hardening already covered (ADR 0295, ADR 0315), and what the
# branch's integrated sub-threads' runs covered (ADR 0465).
#
#   HARDEN_REVIEWED_HEADS="$(lucidos hardened integrated-heads 2>/dev/null)" \
#       ./scripts/harden-scope.sh "$(lucidos hardened sha 2>/dev/null)" [<base>]
#
# HARDEN_REVIEWED_HEADS lists commits other `/harden` runs certified, one per
# line or space. Every commit reachable from one counts as reviewed, exactly as
# a commit on <base> does. A head git cannot resolve is ignored, which only
# widens the review.
#
# The first line is the answer, and the lines after it name what to review:
#
#   MERGE_ONLY       every unreviewed commit after the hardened SHA is a merge
#                    whose other parents bring only reviewed commits
#   INCREMENTAL      the branch has commits of its own since then as well
#
#   commit <sha>     a branch commit since the hardening, oldest first
#   merge <sha>      a merge since the hardening, oldest first
#   overlap <path>   a file both sides of a merge changed
#   other-model-base <sha>
#                    INCREMENTAL with no merge: the other-model review base
#   fork-point <sha> the branch has no marker of its own: its reviewed heads
#                    stand in, and its own work counts from where it forked
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

git rev-parse --verify --quiet "$base^{commit}" >/dev/null || full "base not found: $base"
exclude=("^$base")
for head in ${HARDEN_REVIEWED_HEADS:-}; do
    resolved=$(git rev-parse --verify --quiet "$head^{commit}") && exclude+=("^$resolved")
done

fork=""
if [ -z "$sha" ]; then
    [ "${#exclude[@]}" -gt 1 ] || full "no harden marker"
    sha=$(git merge-base "$base" HEAD) || full "no fork point with $base"
    fork=$sha
fi
git rev-parse --verify --quiet "$sha^{commit}" >/dev/null || full "hardened commit not found: $sha"
git merge-base --is-ancestor "$sha" HEAD || full "hardened commit is not in HEAD's history"
git show --remerge-diff --format= -s HEAD >/dev/null 2>&1 || full "this git has no --remerge-diff"

merges=$(git rev-list --merges --reverse "$sha..HEAD" "${exclude[@]}") || full "git rev-list failed"
for m in $merges; do
    # Every parent after the first must bring only reviewed history, or the
    # merge brought in commits nobody reviewed that this script cannot list.
    side=$(git rev-parse "$m^@" | tail -n +2 | git rev-list --stdin "^$m^1" "${exclude[@]}") ||
        full "git rev-list failed for $m"
    [ -z "$side" ] || full "a merge brought in commits no hardening reviewed"
done
own=$(git rev-list --no-merges --reverse "$sha..HEAD" "${exclude[@]}") || full "git rev-list failed"
[ -n "$fork$own$merges" ] || full "no commit since the last hardening"

# A file both sides of a merge changed since they parted. A semantic conflict
# can merge clean there.
overlap=""
for m in $merges; do
    first=$(git rev-parse "$m^1") || full "git rev-parse failed for $m"
    for other in $(git rev-parse "$m^@" | tail -n +2); do
        mb=$(git merge-base "$first" "$other") || full "no merge base for $m"
        ours=$(git diff --name-only "$mb" "$first" | sort -u) || full "git diff failed for $m"
        theirs=$(git diff --name-only "$mb" "$other" | sort -u) || full "git diff failed for $m"
        overlap="$overlap$(comm -12 <(printf '%s\n' "$ours") <(printf '%s\n' "$theirs"))
"
    done
done

if [ -n "$own" ]; then echo INCREMENTAL; else echo MERGE_ONLY; fi
for c in $own; do
    echo "commit $c"
done
for m in $merges; do
    echo "merge $m"
done
printf '%s' "$overlap" | sort -u | sed -n 's/^\(..*\)$/overlap \1/p'
# With a merge in range, <sha>...HEAD holds all of main's new code as well.
[ -n "$own" ] && [ -z "$merges" ] && echo "other-model-base $sha"
[ -n "$fork" ] && echo "fork-point $fork"
exit 0
