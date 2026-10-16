#!/usr/bin/env bash
#
# adr-new.sh: start a new ADR on a number nobody else has claimed.
#
#   ./scripts/adr-new.sh <short-slug> "<the one-line index text>"
#
# Never pick the number by hand. Reading only `main` is how two branches end up
# claiming the same one, and because their filenames differ git merges them
# CLEANLY, so the collision stays invisible until someone notices two ADRs
# share a number. That has happened twice: 0005 in June, and 0038 on
# 2026-08-04, which had to be renumbered inside a merge conflict resolution.
#
# This allocates across `main`, every local branch not yet merged into it, and
# the working tree. All coding-agent worktrees share one object store and one
# ref namespace, so a sibling session's unmerged ADR is readable from here.
#
# The index text is the line that goes in docs/adr/index.md, and it is
# deliberately allowed to be richer than the heading: it is the whole decision
# in one sentence, because the index is what someone scans before re-opening a
# settled question. Trim the generated heading afterwards if it reads long.
#
# Exit status: 0 created, 1 bad usage or the allocation could not run.

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/lib/adr_scan.sh
source "$SCRIPT_DIR/lib/adr_scan.sh"

usage() {
    awk 'NR == 1 { next } /^#/ { sub(/^# ?/, ""); print; next } { exit }' "${BASH_SOURCE[0]}"
}

case "${1:-}" in
    -h | --help)
        usage
        exit 0
        ;;
esac

if [ $# -ne 2 ]; then
    echo "ERROR: expected a slug and an index line, got $# argument(s)." >&2
    echo >&2
    usage >&2
    exit 1
fi

SLUG="$1"
INDEX_TEXT="$2"

if ! printf '%s' "$SLUG" | grep -qE '^[a-z0-9]+(-[a-z0-9]+)*$'; then
    echo "ERROR: '$SLUG' is not a kebab-case slug (lowercase, digits, single hyphens)." >&2
    exit 1
fi
if [ -z "${INDEX_TEXT// /}" ]; then
    echo "ERROR: the index line text is empty." >&2
    exit 1
fi
case "$INDEX_TEXT" in
    *'['* | *']'* | *'('*)
        # Each breaks the markdown link the index line is built from. `[` is
        # the quiet one: it yields `- [0041: a[b](0041-x.md)`, which the
        # checker still parses as a valid entry, so a malformed line would
        # reach the index unnoticed rather than being reported.
        echo "ERROR: the index line cannot contain '[', ']' or '(' characters." >&2
        exit 1
        ;;
esac

REPO_ROOT="$(git rev-parse --show-toplevel 2> /dev/null)"
if [ -z "$REPO_ROOT" ]; then
    echo "ERROR: not inside a git checkout, so a number cannot be allocated." >&2
    exit 1
fi
if [ ! -f "$REPO_ROOT/$ADR_INDEX" ]; then
    echo "ERROR: $ADR_INDEX is missing, so there is nowhere to record the entry." >&2
    exit 1
fi

# Allocation has to be atomic across worktrees. The scan takes roughly 0.3s, so
# two sessions that both finish scanning before either writes its file would
# receive the SAME number and create it in their separate working trees: the
# silent collision this script exists to prevent, reintroduced at the last
# step. `mkdir` is atomic on every filesystem git runs on, and the common git
# dir is shared by every worktree of the repository, so one lock covers them
# all. It is held across the scan, the file creation, AND the index append.
GIT_COMMON="$(git -C "$REPO_ROOT" rev-parse --git-common-dir 2> /dev/null)"
case "$GIT_COMMON" in
    /*) ;;
    *) GIT_COMMON="$REPO_ROOT/$GIT_COMMON" ;;
esac
LOCK="$GIT_COMMON/lucidos-adr-alloc.lock"
WAITED=0
until mkdir "$LOCK" 2> /dev/null; do
    # A killed session leaves the directory behind. Nothing in here takes
    # anywhere near a minute, so a lock older than that is dead, not slow, and
    # breaking it is safer than making every later allocation hang forever.
    #
    # Break it by RENAMING, never by deleting in place. Two waiters can both
    # see the same stale lock and both decide to clear it; with `rm -rf "$LOCK"`
    # the slower one would delete the directory the faster one had just
    # legitimately re-created, and both would then hold the lock and allocate
    # the same number. `mv` to a per-process name is the atomic claim: the
    # source is already gone for the loser, so exactly one waiter removes
    # exactly the instance it inspected.
    if [ -n "$(find "$LOCK" -maxdepth 0 -mmin +1 2> /dev/null)" ]; then
        if mv "$LOCK" "$LOCK.stale.$$" 2> /dev/null; then
            rm -rf "$LOCK.stale.$$"
        fi
        continue
    fi
    if [ "$WAITED" -ge 30 ]; then
        echo "ERROR: another ADR allocation has held $LOCK for 30s." >&2
        echo "       If no other session is creating an ADR, remove it." >&2
        exit 1
    fi
    sleep 1
    WAITED=$((WAITED + 1))
done
trap 'rm -rf "$LOCK"' EXIT

NUMBER="$(adr_next_number "$REPO_ROOT")" || exit 1

FILENAME="$NUMBER-$SLUG.md"
PATH_NEW="$REPO_ROOT/$ADR_DIR/$FILENAME"
if [ -e "$PATH_NEW" ]; then
    echo "ERROR: $ADR_DIR/$FILENAME already exists." >&2
    exit 1
fi

# The full shape recommended by docs/adr/README.md. check-adrs.sh enforces only
# the subset that every ADR on main already satisfies, but a NEW one should aim
# for all of it, and Alternatives considered is the section the log exists for.
cat > "$PATH_NEW" << EOF
# $NUMBER: $INDEX_TEXT

- **Status**: Accepted
- **Date**: $(date +%Y-%m-%d)

## Context

What prompted the decision.

## Decision

What we chose, in one or two sentences.

## Rationale

Why. This is the part that matters.

## Consequences

What follows from it: what we keep, what we give up.

## Alternatives considered

Each option weighed and why it lost. A rejected option with its reason is worth
more than the chosen one alone.
EOF

printf -- '- [%s: %s](%s)\n' "$NUMBER" "$INDEX_TEXT" "$FILENAME" >> "$REPO_ROOT/$ADR_INDEX"

echo "✓ ADR $NUMBER: $ADR_DIR/$FILENAME"
echo "  indexed in $ADR_INDEX"
echo "  fill in the sections, then ./scripts/check-adrs.sh"
