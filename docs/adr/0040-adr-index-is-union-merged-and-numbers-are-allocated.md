# 0040: Concurrent ADRs stop conflicting: the index is a union-merged file of its own, and a number is allocated across every unmerged branch rather than read off main

- **Status**: Accepted
- **Date**: 2026-08-04

## Context

ADRs were a recurring, time-consuming merge-conflict source. 38 of them were
written in about two months, against a working set of roughly 18 unmerged
coding-agent branches, and the conflicts were not bad luck. Two separate
mechanisms guaranteed them.

**The index was one hand-maintained append-only list inside
`docs/adr/README.md`.** Every new ADR appended a line to the end of the same
list, so two concurrent branches both added a line at the same line number.
That is a same-line add/add conflict by construction: it fires whenever two
ADRs are in flight, which for this repo is most of the time.

**The number was allocated by reading `main`.** Two branches read the same tip
and claimed the same number. Because the filenames differ (`0038-a.md` versus
`0038-b.md`), git merges them *cleanly*, so unlike the index conflict this one
is **silent**. It surfaces later, as a duplicate number nobody notices until a
cross-reference points at the wrong decision. It had already happened twice:

- `4f609a9f5`, "renumber ADR 0005->0006 (0005 taken by codex-app-server on
  main)".
- On 2026-08-04, a branch wrote `0038-the-public-mirror-is-a-linear-release-history.md`
  against a `main` that had meanwhile taken 0038 for
  `0038-a-chat-link-never-leaves-the-workspace.md`. It was hand-renumbered to
  0039 inside the conflict resolution of a merge commit. That placement has a
  second-order cost: a rename that lives only in a merge commit is invisible to
  `git log --name-only`, so the obvious way to ask "which numbers are taken"
  cannot see it.

## Decision

The index moves into `docs/adr/index.md`, a file that is nothing but the list,
and `.gitattributes` gives it `merge=union` so two branches appending a line
both keep it instead of conflicting. Numbers are allocated by
`./scripts/adr-new.sh`, which reads every surface a concurrent branch could have
claimed one from: `main`, every local branch not merged into it, and the working
tree of every attached worktree. The allocation is serialized by a lock in the
shared git directory, so the scan, the file creation and the index append happen
as one step. `./scripts/check-adrs.sh` runs in `/harden` for every diff and
covers what union merge does not.

## Rationale

**Union merge is exactly right for an append-only list, and only for that.** It
keeps both sides' added lines rather than raising a conflict, which is what an
append-only list means. It is also a whole-file attribute, so pointing it at a
file that contains prose means a paragraph reworded on both sides is silently
*duplicated* rather than flagged. That is why the index had to leave `README.md`
rather than have the attribute applied where it sat. `README.md` keeps the
format guidance and gains a link, so every existing reference to
`docs/adr/README.md` still resolves and no sweep was needed.

Both halves were verified rather than assumed. In a scratch repo, two branches
each appending an index line merge cleanly with the attribute present, and
produce `CONFLICT (content)` with the attribute removed and nothing else
changed. The test suite keeps both cases, so an edit that stops the pattern
matching is caught there rather than at the next collision.

**The coordination point git seemed to lack was already there.** Coding-agent
worktrees are `git worktree`s of one repository, so they share an object store
and a ref namespace: a sibling session's unmerged branch is readable from any
other worktree. Scanning `main` plus `git branch --no-merged main` plus the
working tree costs 0.32s over 18 unmerged branches, and sees numbers that no
amount of reading `main` ever will. The allocator proved this on its first real
use: creating this ADR, it returned **0040**, not 0039, because 0039 was held by
an unapplied sibling branch. A read-`main` allocator would have collided a third
time that same day.

The tempting faster alternative, `git log --all --name-only -- docs/adr/`, runs
in 0.06s and returns the wrong answer here. It reported 0038 as the maximum
while 0039 existed, precisely because that renumber lived inside a merge
commit's resolution. Speed is worthless in an allocator that hands out a taken
number, so `adr_scan.sh` reads ref trees and says so at the site.

**Reading every surface is necessary but not sufficient, so allocation is
locked.** Four gaps in the first implementation were each capable of handing out
a taken number, and none was theoretical:

- A sibling worktree's ADR is invisible for the minutes between `adr-new.sh`
  returning a number and the session committing the file. It exists in no ref,
  so only a scan of every attached working tree finds it.
- The branch query hardcoded the name `main` while the precondition accepted
  `refs/heads/main` **or** `refs/remotes/origin/main`. In a clone carrying only
  the remote-tracking ref, `git branch --no-merged main` fails, its stderr is
  discarded, and the scan silently contributes no branches at all.
- A branch fetched but never checked out here exists only as a remote-tracking
  ref, and a local-only branch scan misses it. Seven such branches are unmerged
  in this repo today. Over-reserving is the safe direction: counting an
  abandoned branch's number costs a gap in the sequence, missing a live one
  costs a duplicate.
- The scan itself takes about 0.3s, so two sessions can both finish scanning
  before either writes. Reproduced 5 times out of 5 in the test suite: two
  concurrent invocations produced three files carrying two distinct numbers.

The last is why `adr-new.sh` holds a `mkdir`-based lock in the common git
directory (shared by every worktree) across the scan, the file creation and the
index append. A lock older than a minute is broken rather than waited on, since
nothing in the critical section takes anywhere near that and a session killed
mid-allocation must not wedge every later one.

Breaking it is a *rename*, not a delete. Two waiters can both see the same dead
lock and both decide to clear it, and deleting in place lets the slower one
remove the lock the faster one has just legitimately acquired, putting both
inside the critical section. Renaming to a per-process name succeeds for exactly
one waiter, because the source is already gone for the other. That exclusivity
is tested directly: the end-to-end two-waiter case is a smoke test and does not
reproduce the window, which is only a few instructions wide.

**Union does not order or deduplicate what it keeps.** The same scratch-repo
experiment showed it preserving both lines out of numeric order, and happily
keeping two lines claiming the same number. So the checker is not optional
polish, it is the other half of the design. It runs unconditionally rather than
only when the diff touches `docs/adr/`, because a duplicate that arrives through
a *merge* belongs to nobody's diff, and catching it at any hardening before
Apply is the point.

**A duplicate number is reported, never auto-fixed.** Renumbering means renaming
a file and sweeping its references, and deciding which references are live and
which are historical narration (`CHANGELOG.md`, `docs/plans/`) is a judgment
call. The checker prints both paths and the next free number; a human or agent
does the rename. `--fix` only restores order, and is tested to be idempotent and
to leave every line's text byte-identical.

## Consequences

- Two ADRs written at once no longer conflict. The index merge is clean and both
  entries survive.
- A number collision is now close to impossible, and loud rather than silent if
  one happens anyway.
- The index is a second file. `docs/adr/README.md` no longer shows the list, it
  links to it.
- `merge=union` is deliberately scoped to one path. Widening it to any file
  containing prose reintroduces silent duplication, and a test asserts that no
  prose file inherits a merge driver.
- The checker enforces a **subset** of the shape `README.md` recommends:
  `## Context`, `## Decision`, `## Consequences`, a Status line, and a heading
  numbered to match the filename. Rationale and Alternatives considered stay
  recommended and unenforced, because house style has evolved to fold reasoning
  into custom "## Why ..." sections and 15 of the 38 existing ADRs carry no
  `## Rationale` heading. A gate that fails on 40% of the tree it guards teaches
  people to ignore it.
- Writing an ADR by hand still works and is not blocked. It is the *numbering*
  that must not be done by hand, and both `CLAUDE.md` and `docs/adr/README.md`
  say so at the point where an agent is about to write one.

## Alternatives considered

**Drop the number from ADR filenames and identify decisions by slug.** This
removes the allocation race outright rather than coordinating it, and is what
`docs/plans/` already does with dates. Rejected because "ADR 0014" is
load-bearing shorthand embedded in `CLAUDE.md`, `.claude/rules/*`, engine source
comments, `CHANGELOG.md` and roughly thirty plan documents. Renaming the files
breaks every one of those links, and the references inside `CHANGELOG.md` and
`docs/plans/` are historical narration that must not be rewritten to match.

**Apply `merge=union` to `README.md` where the index already was.** One line of
configuration and no new file. Rejected because the attribute is per-file: the
same rule that keeps both index lines would keep both versions of a reworded
paragraph, and a silently duplicated sentence in prose is worse than the
conflict it avoided.

**Generate the index from each ADR's `# NNNN: ...` heading.** Then the index is
derived, and a conflict in it is resolved by re-running the generator. Rejected
because the index lines are deliberately richer than the headings: ADR 0038's
heading is "A chat link never leaves the workspace", while its index line
continues "the click handler's extractor chain is closed at the bottom, so an
unclaimed href is a toast rather than an SPA-fallback reload". The index is what
someone scans before re-opening a settled question, and generation would discard
exactly the part that makes it worth scanning.

**Delete the index entirely and let the directory listing serve.** Zero
maintenance and zero conflicts. Rejected for the same reason: 38 one-line
decision statements in one file is a genuinely useful artifact both for a human
skimming and for an LLM loading context, and a list of filenames is not.

**Coordinate allocation through the engine, with a counter in Postgres.** The
engine is a real coordination point that git lacks, and it would be exact.
Rejected because ADR numbers are a property of the repository, which outlives
any one workspace database and is published to a public mirror where a
contributor has no engine running. Reading sibling refs achieves the same result
with nothing but git.

**Leave the numbering alone and let the checker catch duplicates.** Cheaper, and
the collision does become loud. Rejected as a half-fix: it converts a silent
problem into a visible one without removing the renumbering work, which is the
part that actually costs time.
