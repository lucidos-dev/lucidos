#!/usr/bin/env bash
#
# adr_scan.sh: the shared half of the ADR tooling. Sourced by
# scripts/adr-new.sh (allocate a number, scaffold an entry) and
# scripts/check-adrs.sh (verify the result, restore index order).
#
# Two problems shape everything here, both recorded in
# docs/plans/2026-08-04-conflict-free-adr-index-and-numbering.md:
#
#   1. docs/adr/index.md is append-only and carries `merge=union`, so two
#      branches adding an ADR at once both keep their line instead of
#      conflicting. Union does not order or deduplicate what it keeps, which is
#      why adr_index_sort and the duplicate check below exist.
#   2. A number read off `main` alone collides silently, because two ADRs with
#      different filenames merge clean. adr_claimed_numbers therefore reads
#      every surface a concurrent branch could have claimed one from.
#
# Not executable on its own.

ADR_DIR="docs/adr"
ADR_INDEX="docs/adr/index.md"

# The sections a new ADR must carry. Deliberately NOT the full recommended
# shape in docs/adr/README.md, which also asks for Rationale and Alternatives
# considered: house style has since evolved to fold the reasoning into custom
# "## Why ..." sections, so 15 of the 38 ADRs on main carry no "## Rationale"
# heading and 14 no "## Alternatives considered". Those stay recommended in the
# README and unenforced here. A gate that fails on 40% of the tree it guards is
# a gate nobody can keep green, and a permanently red gate teaches people to
# ignore it.
# shellcheck disable=SC2034 # read by scripts/check-adrs.sh, which sources this
ADR_REQUIRED_SECTIONS=("## Context" "## Decision" "## Consequences")

# Every ADR file in one working tree, as bare filenames, sorted. Expands the
# path in the shell rather than forking basename, because adr_claimed_numbers
# calls this once per worktree and there can be twenty of them.
adr_files() {
    local root="$1" path
    for path in "$root/$ADR_DIR"/[0-9][0-9][0-9][0-9]-*.md; do
        [ -e "$path" ] || continue
        echo "${path##*/}"
    done | LC_ALL=C sort
}

# The ref that "has this branch landed yet" is asked against. A clone can carry
# `origin/main` with no local `main`, and asking `git branch --no-merged main`
# there fails and contributes NO branches: the allocator would then miss every
# sibling branch's ADR and hand out a taken number, silently. Resolving the ref
# once keeps the precondition and the branch query from disagreeing.
adr_main_ref() {
    local root="$1" ref
    for ref in refs/heads/main refs/remotes/origin/main; do
        if git -C "$root" rev-parse --verify -q "$ref" > /dev/null; then
            echo "$ref"
            return 0
        fi
    done
    return 1
}

# The refs a concurrent branch could be holding an unmerged ADR on. The main ref
# is the floor; every branch not yet merged into it is the point, since all
# coding-agent worktrees share one object store and one ref namespace, so a
# sibling session's branch is readable from here. HEAD covers a detached
# checkout, which `git branch` would not list.
#
# `-a`, not local-only: a branch that was fetched but never checked out here
# exists solely as a remote-tracking ref, and an ADR on it is just as taken as
# one on a local branch. Seven such branches are unmerged in this repo today.
# Over-reserving is the safe direction anyway, since the cost of counting an
# abandoned branch's number is a gap in the sequence, while the cost of missing
# a live one is the silent duplicate this whole file exists to prevent.
adr_scan_refs() {
    local root="$1" main_ref
    main_ref="$(adr_main_ref "$root")" || return 1
    echo "$main_ref"
    echo HEAD
    git -C "$root" branch -a --no-merged "$main_ref" --format='%(refname)' 2> /dev/null
}

# Every working tree attached to this repository, this one included. A sibling
# coding-agent session can be holding a freshly allocated ADR that is not yet
# committed anywhere, so it appears in no ref at all, and minutes pass between
# allocating a number and committing it. Scanning only our own working tree
# would leave exactly the collision window this tooling exists to close.
adr_worktree_roots() {
    git -C "$1" worktree list --porcelain 2> /dev/null | sed -n 's/^worktree //p'
}

# Every 4-digit number already claimed, anywhere it could have been: every
# attached working tree (uncommitted files included) plus each scanned ref.
#
# Do NOT be tempted to replace this with `git log --all --name-only`. It is 5x
# faster and wrong: on 2026-08-04 it reported 0038 as the maximum while 0039
# existed on a sibling branch, because that renumber lived only inside a merge
# commit's conflict resolution and no ordinary name-only walk shows it.
adr_claimed_numbers() {
    local root="$1" ref worktree
    {
        adr_files "$root"
        while read -r worktree; do
            [ -n "$worktree" ] && [ -d "$worktree" ] || continue
            adr_files "$worktree"
        done < <(adr_worktree_roots "$root")
        while read -r ref; do
            [ -n "$ref" ] || continue
            git -C "$root" ls-tree -r --name-only "$ref" -- "$ADR_DIR" 2> /dev/null
        done < <(adr_scan_refs "$root")
    } | sed 's|.*/||' | grep -oE '^[0-9]{4}' | LC_ALL=C sort -u
}

# The next free number, zero-padded. Non-zero exit means the scan could not
# run, which must never be reported as "0001 is free".
adr_next_number() {
    local root="$1" highest
    if ! adr_main_ref "$root" > /dev/null; then
        echo "ERROR: neither refs/heads/main nor refs/remotes/origin/main resolves," >&2
        echo "       so the set of claimed ADR numbers cannot be established." >&2
        return 1
    fi
    highest="$(adr_claimed_numbers "$root" | tail -1)"
    [ -n "$highest" ] || highest="0000"
    # Force base 10: a leading zero makes bash read the literal as octal, so
    # 0008 and 0009 would both be "invalid octal" errors.
    if [ "$((10#$highest))" -ge 9999 ]; then
        echo "ERROR: ADR numbers are exhausted at $highest." >&2
        return 1
    fi
    printf '%04d\n' "$((10#$highest + 1))"
}

# Index entry lines, verbatim and in file order.
adr_index_entries() {
    local root="$1"
    [ -f "$root/$ADR_INDEX" ] || return 0
    grep -E '^- \[' "$root/$ADR_INDEX"
}

# The number an entry line claims.
adr_entry_number() {
    printf '%s\n' "$1" | sed -E 's/^- \[([0-9]{4}).*/\1/'
}

# The file an entry line links to. Anchored on the LAST "](" in the line, not
# the first "(": a title may itself contain parentheses (0036 ends "(drafts
# emit no `release` event)"), and two entries carry a trailing note after the
# link (0007, 0013).
adr_entry_target() {
    printf '%s\n' "$1" | sed -E 's/.*\]\(([^)]*)\).*/\1/'
}
