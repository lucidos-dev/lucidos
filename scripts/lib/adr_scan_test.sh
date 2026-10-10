#!/bin/bash
# Tests for the ADR conflict tooling: the shared library scripts/lib/adr_scan.sh,
# the allocator scripts/adr-new.sh, the checker scripts/check-adrs.sh, and the
# `merge=union` attribute in .gitattributes that the whole design rests on.
#
# Hermetic: every case builds a throwaway git repo under mktemp -d and runs the
# real scripts inside it. Nothing reads or writes the Lucidos tree, and no
# process is signalled, so the outcome cannot drift as the repo gains ADRs.
#
# The two behaviours worth stating up front, because they are the reason the
# tooling is shaped this way (see
# docs/plans/2026-08-04-conflict-free-adr-index-and-numbering.md):
#
#   - `merge=union` turns the guaranteed append/append conflict on the index
#     into a clean merge. The control case removes ONLY the attribute and
#     asserts the same merge conflicts, so a future edit to .gitattributes that
#     silently stops matching is caught here rather than at the next collision.
#   - Union keeps both lines but neither orders nor deduplicates them, which is
#     precisely the gap check-adrs.sh covers.
#
# Run: ./scripts/lib/adr_scan_test.sh
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_SRC="$(cd "$SCRIPT_DIR/../.." && pwd)"
NEW="$REPO_SRC/scripts/adr-new.sh"
CHECK="$REPO_SRC/scripts/check-adrs.sh"
ATTRS_LINE="docs/adr/index.md merge=union"

PASS=0
FAIL=0
pass() { echo "  ok:   $*"; PASS=$((PASS + 1)); }
fail() { echo "  FAIL: $*"; FAIL=$((FAIL + 1)); }

TMP_ROOT="$(mktemp -d)"
trap 'rm -rf "$TMP_ROOT"' EXIT

# A fresh fixture repo on `main` with one valid ADR and an index. Echoes its
# path; each test gets its own so cases cannot contaminate each other.
new_repo() {
    local repo
    repo="$(mktemp -d "$TMP_ROOT/repo.XXXXXX")"
    git -C "$repo" init -q -b main
    git -C "$repo" config user.email "t@t"
    git -C "$repo" config user.name "t"
    mkdir -p "$repo/docs/adr"
    printf '%s\n' "$ATTRS_LINE" > "$repo/.gitattributes"
    write_adr "$repo" "0001" "first-decision"
    {
        printf '# ADR index\n\n'
        printf -- '- [0001: First decision](0001-first-decision.md)\n'
    } > "$repo/docs/adr/index.md"
    git -C "$repo" add -A
    git -C "$repo" commit -qm base
    echo "$repo"
}

# A minimally valid ADR: heading numbered to match the filename, a Status line,
# and the three sections check-adrs.sh enforces.
write_adr() { # <repo> <number> <slug>
    cat > "$1/docs/adr/$2-$3.md" << EOF
# $2: A decision

- **Status**: Accepted
- **Date**: 2026-01-01

## Context

Context.

## Decision

Decision.

## Consequences

Consequences.
EOF
}

adr_count() { # <repo>
    local path n=0
    for path in "$1"/docs/adr/[0-9][0-9][0-9][0-9]-*.md; do
        [ -e "$path" ] || continue
        n=$((n + 1))
    done
    echo "$n"
}

index_line() { # <repo> <number> <slug>
    printf -- '- [%s: A decision](%s-%s.md)\n' "$2" "$2" "$3" >> "$1/docs/adr/index.md"
}

run_check() { # <repo> [args...]
    local repo="$1"
    shift
    CHECK_OUT=$(cd "$repo" && bash "$CHECK" "$@" 2>&1)
    CHECK_RC=$?
}

run_new() { # <repo> [args...]
    local repo="$1"
    shift
    NEW_OUT=$(cd "$repo" && bash "$NEW" "$@" 2>&1)
    NEW_RC=$?
}

# ---------------------------------------------------------------- union merge

test_union_merge_keeps_both_appended_lines() {
    local repo
    repo="$(new_repo)"
    git -C "$repo" checkout -q -b branch-a
    write_adr "$repo" "0002" "alpha"
    index_line "$repo" "0002" "alpha"
    git -C "$repo" add -A && git -C "$repo" commit -qm alpha

    git -C "$repo" checkout -q main
    git -C "$repo" checkout -q -b branch-b
    write_adr "$repo" "0003" "bravo"
    index_line "$repo" "0003" "bravo"
    git -C "$repo" add -A && git -C "$repo" commit -qm bravo

    if git -C "$repo" merge branch-a --no-edit -q > /dev/null 2>&1; then
        if grep -q '0002-alpha' "$repo/docs/adr/index.md" \
            && grep -q '0003-bravo' "$repo/docs/adr/index.md"; then
            pass "concurrent index appends merge clean and both lines survive"
        else
            fail "merge was clean but an index line was lost"
        fi
    else
        fail "concurrent index appends conflicted despite merge=union"
    fi
}

test_control_without_the_attribute_conflicts() {
    local repo
    repo="$(new_repo)"
    # Remove ONLY the attribute. If this case ever passes, merge=union stopped
    # being what makes the case above work and the shipped attribute is dead.
    rm "$repo/.gitattributes"
    git -C "$repo" add -A && git -C "$repo" commit -qm "drop attribute"

    git -C "$repo" checkout -q -b branch-a
    index_line "$repo" "0002" "alpha"
    git -C "$repo" commit -qam alpha
    git -C "$repo" checkout -q main
    git -C "$repo" checkout -q -b branch-b
    index_line "$repo" "0003" "bravo"
    git -C "$repo" commit -qam bravo

    if git -C "$repo" merge branch-a --no-edit -q > /dev/null 2>&1; then
        fail "control: the same merge should conflict without merge=union"
    else
        git -C "$repo" merge --abort 2> /dev/null
        pass "control: without merge=union the identical merge conflicts"
    fi
}

test_shipped_gitattributes_carries_the_rule() {
    if grep -qxF "$ATTRS_LINE" "$REPO_SRC/.gitattributes" 2> /dev/null; then
        pass "the shipped .gitattributes carries the index union rule"
    else
        fail "the shipped .gitattributes no longer carries '$ATTRS_LINE'"
    fi
}

test_union_is_scoped_to_the_index_alone() {
    local other stray
    stray=""
    for other in docs/adr/README.md docs/glossary.md CHANGELOG.md CLAUDE.md; do
        if [ "$(git -C "$REPO_SRC" check-attr merge -- "$other" | sed 's/.*: //')" != "unspecified" ]; then
            stray="$stray $other"
        fi
    done
    if [ -z "$stray" ]; then
        pass "no prose file inherits a merge driver"
    else
        fail "a merge driver reaches prose:$stray"
    fi
}

# ----------------------------------------------------------------- allocation

test_allocates_above_main_branch_and_worktree() {
    local repo out
    repo="$(new_repo)"
    # 0002 on an unmerged branch...
    git -C "$repo" checkout -q -b sibling
    write_adr "$repo" "0002" "sibling"
    index_line "$repo" "0002" "sibling"
    git -C "$repo" add -A && git -C "$repo" commit -qm sibling
    git -C "$repo" checkout -q main
    # ...and 0003 uncommitted in the working tree.
    write_adr "$repo" "0003" "uncommitted"

    run_new "$repo" "fourth" "A fourth decision"
    out="$NEW_OUT"
    if [ "$NEW_RC" -eq 0 ] && [ -f "$repo/docs/adr/0004-fourth.md" ]; then
        pass "allocates above main, an unmerged branch, and the working tree"
    else
        fail "expected 0004, got rc=$NEW_RC: $out"
    fi
}

test_allocates_above_a_number_renamed_inside_a_merge_commit() {
    local repo
    repo="$(new_repo)"
    git -C "$repo" checkout -q -b branch-a
    write_adr "$repo" "0002" "alpha"
    index_line "$repo" "0002" "alpha"
    git -C "$repo" add -A && git -C "$repo" commit -qm alpha

    git -C "$repo" checkout -q main
    git -C "$repo" checkout -q -b branch-b
    write_adr "$repo" "0002" "bravo"
    index_line "$repo" "0002" "bravo"
    git -C "$repo" add -A && git -C "$repo" commit -qm bravo

    # Resolve the collision the way it was resolved on 2026-08-04: renumber
    # INSIDE the merge commit. The renamed path then exists in no ordinary
    # name-only history walk, which is why the allocator reads ref trees.
    git -C "$repo" merge branch-a --no-commit --no-ff -q > /dev/null 2>&1
    git -C "$repo" mv docs/adr/0002-bravo.md docs/adr/0003-bravo.md
    sed 's|0002: A decision](0002-bravo|0003: A decision](0003-bravo|' \
        "$repo/docs/adr/index.md" > "$repo/docs/adr/index.tmp"
    mv "$repo/docs/adr/index.tmp" "$repo/docs/adr/index.md"
    git -C "$repo" add -A && git -C "$repo" commit -qm "merge + renumber"

    git -C "$repo" checkout -q main
    run_new "$repo" "fourth" "A fourth decision"
    if [ "$NEW_RC" -eq 0 ] && [ -f "$repo/docs/adr/0004-fourth.md" ]; then
        pass "allocates above a number renamed inside a merge commit"
    else
        fail "expected 0004 past the merge-commit rename, rc=$NEW_RC: $NEW_OUT"
    fi
}

test_allocates_above_a_sibling_worktree_that_has_not_committed() {
    local repo sibling
    repo="$(new_repo)"
    sibling="$repo-wt"
    git -C "$repo" worktree add -q -b sibling-branch "$sibling" > /dev/null 2>&1
    # Allocated in the sibling but not yet committed, which is the ordinary
    # state for the minutes between adr-new.sh and the commit. It appears in no
    # ref, so only a working-tree scan of the sibling can see it.
    write_adr "$sibling" "0002" "in-flight"

    run_new "$repo" "third" "A third decision"
    if [ "$NEW_RC" -eq 0 ] && [ -f "$repo/docs/adr/0003-third.md" ]; then
        pass "allocates above an uncommitted ADR in a sibling worktree"
    else
        fail "expected 0003 past the sibling worktree, rc=$NEW_RC: $NEW_OUT"
    fi
}

test_allocates_correctly_with_no_local_main() {
    local origin clone
    origin="$(new_repo)"
    clone="$(mktemp -d "$TMP_ROOT/clone.XXXXXX")/c"
    git clone -q "$origin" "$clone"
    git -C "$clone" config user.email "t@t"
    git -C "$clone" config user.name "t"
    # A sibling local branch holding 0002...
    git -C "$clone" checkout -q -b sibling
    write_adr "$clone" "0002" "sibling"
    index_line "$clone" "0002" "sibling"
    git -C "$clone" add -A && git -C "$clone" commit -qm sibling
    # ...and no local main at all, only origin/main. `git branch --no-merged
    # main` fails here, so an allocator that hardcodes the name silently sees
    # no sibling branches and reuses 0002.
    #
    # Detach at origin/main, NOT at the sibling tip: parking HEAD on the
    # sibling commit would let the HEAD entry in the ref set cover for the
    # broken branch query, and the case would pass against the very bug it
    # exists to catch.
    git -C "$clone" checkout -q --detach origin/main
    git -C "$clone" branch -q -D main

    run_new "$clone" "third" "A third decision"
    if [ "$NEW_RC" -eq 0 ] && [ -f "$clone/docs/adr/0003-third.md" ]; then
        pass "allocates against origin/main when there is no local main"
    else
        fail "expected 0003 with no local main, rc=$NEW_RC: $NEW_OUT"
    fi
}

test_allocates_above_a_fetched_remote_branch() {
    local origin clone
    origin="$(new_repo)"
    clone="$(mktemp -d "$TMP_ROOT/rclone.XXXXXX")/c"
    git clone -q "$origin" "$clone"
    git -C "$clone" config user.email "t@t"
    git -C "$clone" config user.name "t"
    # 0002 lands on a branch in the origin, then is fetched. In the clone it
    # exists ONLY as a remote-tracking ref: never checked out, no local branch,
    # so a local-only branch scan cannot see it.
    git -C "$origin" checkout -q -b pushed-elsewhere
    write_adr "$origin" "0002" "elsewhere"
    index_line "$origin" "0002" "elsewhere"
    git -C "$origin" add -A && git -C "$origin" commit -qm elsewhere
    git -C "$origin" checkout -q main
    git -C "$clone" fetch -q origin

    run_new "$clone" "third" "A third decision"
    if [ "$NEW_RC" -eq 0 ] && [ -f "$clone/docs/adr/0003-third.md" ]; then
        pass "allocates above an ADR on a fetched remote-tracking branch"
    else
        fail "expected 0003 past the remote branch, rc=$NEW_RC: $NEW_OUT"
    fi
}

test_concurrent_allocation_does_not_collide() {
    local repo distinct total
    repo="$(new_repo)"
    # The window the lock closes: both scans complete before either write.
    (cd "$repo" && bash "$NEW" "racer-one" "One" > /dev/null 2>&1) &
    (cd "$repo" && bash "$NEW" "racer-two" "Two" > /dev/null 2>&1) &
    wait
    total="$(adr_count "$repo")"
    distinct="$(cd "$repo/docs/adr" && for f in [0-9][0-9][0-9][0-9]-*.md; do
        echo "${f%%-*}"
    done | sort -u | grep -c '')"
    if [ "$total" -eq 3 ] && [ "$distinct" -eq 3 ]; then
        pass "two concurrent allocations take two different numbers"
    else
        fail "concurrent allocation: $total files but $distinct distinct numbers"
    fi
}

stale_lock_path() { # <repo>
    local lock
    lock="$(git -C "$1" rev-parse --git-common-dir)/lucidos-adr-alloc.lock"
    case "$lock" in
        /*) ;;
        *) lock="$1/$lock" ;;
    esac
    echo "$lock"
}

# Two minutes old: a session that died holding it.
age_lock() { touch -t "$(date -v-2M +%Y%m%d%H%M 2> /dev/null || date -d '2 minutes ago' +%Y%m%d%H%M)" "$1"; }

test_stale_lock_claim_is_exclusive() {
    local dir first second
    dir="$(mktemp -d "$TMP_ROOT/lock.XXXXXX")/l"
    mkdir -p "$dir"
    # The property the stale-lock recovery rests on, tested directly because
    # the process-level race below is too narrow to reproduce on demand:
    # renaming ONE directory to two different names succeeds exactly once,
    # since the source is already gone for the loser. `rm -rf "$LOCK"` has no
    # such exclusivity, which is how a second waiter could delete the lock a
    # first waiter had legitimately just acquired, putting both inside the
    # critical section.
    if mv "$dir" "$dir.a" 2> /dev/null; then first=ok; else first=no; fi
    if mv "$dir" "$dir.b" 2> /dev/null; then second=ok; else second=no; fi
    if [ "$first" = ok ] && [ "$second" = no ]; then
        pass "claiming a stale lock by rename succeeds for exactly one waiter"
    else
        fail "the rename claim was not exclusive: first=$first second=$second"
    fi
}

# End-to-end smoke test over the recovery path. It does NOT reliably reproduce
# the delete-in-place race (that window is a few instructions wide, and this
# passed against the buggy version 6 runs out of 6); the exclusivity property
# above is what actually guards it. Kept because it exercises staleness
# detection, recovery and allocation together.
test_two_waiters_on_a_stale_lock_still_allocate() {
    local repo lock distinct total
    repo="$(new_repo)"
    lock="$(stale_lock_path "$repo")"
    mkdir -p "$lock"
    age_lock "$lock"
    (cd "$repo" && bash "$NEW" "waiter-one" "One" > /dev/null 2>&1) &
    (cd "$repo" && bash "$NEW" "waiter-two" "Two" > /dev/null 2>&1) &
    wait
    total="$(adr_count "$repo")"
    distinct="$(cd "$repo/docs/adr" && for f in [0-9][0-9][0-9][0-9]-*.md; do
        echo "${f%%-*}"
    done | sort -u | grep -c '')"
    if [ "$total" -eq 3 ] && [ "$distinct" -eq 3 ]; then
        pass "two waiters past one stale lock both allocate, with different numbers"
    else
        fail "stale-lock recovery: $total files but $distinct numbers"
    fi
}

test_a_stale_lock_is_broken() {
    local repo lock
    repo="$(new_repo)"
    lock="$(stale_lock_path "$repo")"
    mkdir -p "$lock"
    # Waiting forever on a dead lock would be worse than the race it guards.
    age_lock "$lock"
    run_new "$repo" "after-stale" "After a stale lock"
    if [ "$NEW_RC" -eq 0 ] && [ -f "$repo/docs/adr/0002-after-stale.md" ]; then
        pass "a stale lock is broken rather than waited on forever"
    else
        fail "stale lock not broken, rc=$NEW_RC: $NEW_OUT"
    fi
}

test_new_entry_passes_the_checker() {
    local repo
    repo="$(new_repo)"
    run_new "$repo" "second-decision" "A second decision that is entirely fine"
    run_check "$repo"
    if [ "$NEW_RC" -eq 0 ] && [ "$CHECK_RC" -eq 0 ]; then
        pass "a freshly allocated ADR passes the checker unedited"
    else
        fail "new rc=$NEW_RC check rc=$CHECK_RC: $NEW_OUT / $CHECK_OUT"
    fi
}

test_rejects_a_bad_slug_and_an_empty_line() {
    local repo ok
    repo="$(new_repo)"
    ok=1
    run_new "$repo" "Not Kebab" "Text"
    [ "$NEW_RC" -eq 1 ] || ok=0
    run_new "$repo" "fine-slug" "   "
    [ "$NEW_RC" -eq 1 ] || ok=0
    run_new "$repo" "fine-slug" "text with (parens) that would break the link"
    [ "$NEW_RC" -eq 1 ] || ok=0
    # Each bracket separately: `[` is the one that still parses as a valid
    # entry afterwards, so omitting it from the guard is silent.
    run_new "$repo" "fine-slug" "text with an opening [ bracket"
    [ "$NEW_RC" -eq 1 ] || ok=0
    run_new "$repo" "fine-slug" "text with a closing ] bracket"
    [ "$NEW_RC" -eq 1 ] || ok=0
    run_new "$repo" "only-one-arg"
    [ "$NEW_RC" -eq 1 ] || ok=0
    if [ "$ok" -eq 1 ]; then
        pass "rejects a bad slug, an empty line, every link-breaking character, and a missing arg"
    else
        fail "a malformed invocation was accepted"
    fi
}

# -------------------------------------------------------------------- checker

test_clean_tree_passes() {
    local repo
    repo="$(new_repo)"
    run_check "$repo"
    if [ "$CHECK_RC" -eq 0 ]; then
        pass "a consistent directory and index pass"
    else
        fail "clean fixture reported problems: $CHECK_OUT"
    fi
}

test_duplicate_number_is_reported() {
    local repo
    repo="$(new_repo)"
    write_adr "$repo" "0002" "alpha"
    index_line "$repo" "0002" "alpha"
    write_adr "$repo" "0002" "bravo"
    index_line "$repo" "0002" "bravo"
    run_check "$repo"
    if [ "$CHECK_RC" -eq 1 ] && printf '%s' "$CHECK_OUT" | grep -q "duplicate number 0002"; then
        pass "two files sharing a number are reported"
    else
        fail "duplicate number not reported, rc=$CHECK_RC: $CHECK_OUT"
    fi
}

test_duplicate_number_is_never_auto_fixed() {
    local repo
    repo="$(new_repo)"
    write_adr "$repo" "0002" "alpha"
    index_line "$repo" "0002" "alpha"
    write_adr "$repo" "0002" "bravo"
    index_line "$repo" "0002" "bravo"
    run_check "$repo" --fix
    if [ "$CHECK_RC" -eq 1 ] \
        && [ -f "$repo/docs/adr/0002-alpha.md" ] \
        && [ -f "$repo/docs/adr/0002-bravo.md" ]; then
        pass "--fix reports a duplicate number without renaming anything"
    else
        fail "--fix touched a duplicate, rc=$CHECK_RC: $CHECK_OUT"
    fi
}

test_file_without_an_index_line_is_reported() {
    local repo
    repo="$(new_repo)"
    write_adr "$repo" "0002" "orphan"
    run_check "$repo"
    if [ "$CHECK_RC" -eq 1 ] && printf '%s' "$CHECK_OUT" | grep -q "0002-orphan.md has no line"; then
        pass "an ADR with no index line is reported"
    else
        fail "orphaned file not reported, rc=$CHECK_RC: $CHECK_OUT"
    fi
}

test_index_line_without_a_file_is_reported() {
    local repo
    repo="$(new_repo)"
    index_line "$repo" "0002" "ghost"
    run_check "$repo"
    if [ "$CHECK_RC" -eq 1 ] && printf '%s' "$CHECK_OUT" | grep -q "does not exist"; then
        pass "an index line with no file is reported"
    else
        fail "orphaned index line not reported, rc=$CHECK_RC: $CHECK_OUT"
    fi
}

test_heading_number_must_match_the_filename() {
    local repo
    repo="$(new_repo)"
    write_adr "$repo" "0002" "mismatch"
    index_line "$repo" "0002" "mismatch"
    sed 's/^# 0002:/# 0009:/' "$repo/docs/adr/0002-mismatch.md" > "$repo/docs/adr/t"
    mv "$repo/docs/adr/t" "$repo/docs/adr/0002-mismatch.md"
    run_check "$repo"
    if [ "$CHECK_RC" -eq 1 ] && printf '%s' "$CHECK_OUT" | grep -q "heading opening with 0002"; then
        pass "a heading numbered differently from its filename is reported"
    else
        fail "heading mismatch not reported, rc=$CHECK_RC: $CHECK_OUT"
    fi
}

test_missing_section_is_reported() {
    local repo
    repo="$(new_repo)"
    write_adr "$repo" "0002" "thin"
    index_line "$repo" "0002" "thin"
    grep -v '^## Decision$' "$repo/docs/adr/0002-thin.md" > "$repo/docs/adr/t"
    mv "$repo/docs/adr/t" "$repo/docs/adr/0002-thin.md"
    run_check "$repo"
    if [ "$CHECK_RC" -eq 1 ] && printf '%s' "$CHECK_OUT" | grep -q "no '## Decision' section"; then
        pass "a missing required section is reported"
    else
        fail "missing section not reported, rc=$CHECK_RC: $CHECK_OUT"
    fi
}

test_missing_index_fails_closed() {
    local repo
    repo="$(new_repo)"
    rm "$repo/docs/adr/index.md"
    run_check "$repo"
    if [ "$CHECK_RC" -eq 1 ] && printf '%s' "$CHECK_OUT" | grep -q "cannot run"; then
        pass "a missing index fails closed rather than reporting clean"
    else
        fail "missing index did not fail closed, rc=$CHECK_RC: $CHECK_OUT"
    fi
}

# ------------------------------------------------------------------ --fix

# Build a repo whose index is in the state a union merge leaves behind: both
# lines present, the later number first.
unsorted_repo() {
    local repo
    repo="$(new_repo)"
    write_adr "$repo" "0002" "alpha"
    write_adr "$repo" "0003" "bravo"
    index_line "$repo" "0003" "bravo"
    index_line "$repo" "0002" "alpha"
    echo "$repo"
}

test_out_of_order_is_reported_then_fixed() {
    local repo
    repo="$(unsorted_repo)"
    run_check "$repo"
    if [ "$CHECK_RC" -ne 1 ] || ! printf '%s' "$CHECK_OUT" | grep -q "out of order"; then
        fail "out-of-order index not reported, rc=$CHECK_RC: $CHECK_OUT"
        return
    fi
    run_check "$repo" --fix
    if [ "$CHECK_RC" -eq 0 ]; then
        pass "an out-of-order index is reported, then repaired by --fix"
    else
        fail "--fix did not repair the order: $CHECK_OUT"
    fi
}

test_fix_is_idempotent() {
    local repo
    repo="$(unsorted_repo)"
    run_check "$repo" --fix
    cp "$repo/docs/adr/index.md" "$repo/once"
    run_check "$repo" --fix
    if diff -q "$repo/once" "$repo/docs/adr/index.md" > /dev/null; then
        pass "--fix run twice matches --fix run once"
    else
        fail "--fix is not idempotent"
    fi
}

test_fix_only_reorders() {
    local repo before after
    repo="$(unsorted_repo)"
    before="$(sort "$repo/docs/adr/index.md")"
    run_check "$repo" --fix
    after="$(sort "$repo/docs/adr/index.md")"
    if [ "$before" = "$after" ]; then
        pass "--fix reorders without adding, dropping, or rewording a line"
    else
        fail "--fix changed the set of lines"
    fi
}

test_fix_refuses_to_strand_content() {
    local repo
    repo="$(unsorted_repo)"
    printf 'A trailing paragraph that is not an entry.\n' >> "$repo/docs/adr/index.md"
    run_check "$repo" --fix
    if [ "$CHECK_RC" -eq 1 ] \
        && printf '%s' "$CHECK_OUT" | grep -q "refuses to reorder" \
        && grep -q "trailing paragraph" "$repo/docs/adr/index.md"; then
        pass "--fix refuses to reorder around content it would strand"
    else
        fail "--fix did not protect stray content, rc=$CHECK_RC: $CHECK_OUT"
    fi
}

echo "── merge=union ──"
test_shipped_gitattributes_carries_the_rule
test_union_is_scoped_to_the_index_alone
test_union_merge_keeps_both_appended_lines
test_control_without_the_attribute_conflicts

echo "── allocation ──"
test_allocates_above_main_branch_and_worktree
test_allocates_above_a_number_renamed_inside_a_merge_commit
test_allocates_above_a_sibling_worktree_that_has_not_committed
test_allocates_correctly_with_no_local_main
test_allocates_above_a_fetched_remote_branch
test_concurrent_allocation_does_not_collide
test_a_stale_lock_is_broken
test_stale_lock_claim_is_exclusive
test_two_waiters_on_a_stale_lock_still_allocate
test_new_entry_passes_the_checker
test_rejects_a_bad_slug_and_an_empty_line

echo "── checker ──"
test_clean_tree_passes
test_duplicate_number_is_reported
test_duplicate_number_is_never_auto_fixed
test_file_without_an_index_line_is_reported
test_index_line_without_a_file_is_reported
test_heading_number_must_match_the_filename
test_missing_section_is_reported
test_missing_index_fails_closed

echo "── --fix ──"
test_out_of_order_is_reported_then_fixed
test_fix_is_idempotent
test_fix_only_reorders
test_fix_refuses_to_strand_content

echo
echo "passed: $PASS   failed: $FAIL"
[ "$FAIL" -eq 0 ]
