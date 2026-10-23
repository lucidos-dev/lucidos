# 0458: An archived thread's worktree is removed an hour after it idles; dirt is committed on its own branch first, unmerged branches stay, and removal moves the tree aside before deleting it

- **Status**: Accepted
- **Date**: 2026-10-12

## Context

Archived coding-agent threads kept their worktrees. One dev workspace held 42
worktrees and 279 GB, 12 of them archived, some for a month.

Archive already opened the cleanup worker's *retention gate* (ADR 0310), and
Tier 1 stripped build artifacts a day later. The full-removal tiers stopped
there:

- Tier 0 removed only a *finished worktree*: clean, and no commits ahead of the
  local default branch.
- Tier 2 waited 30 days, and took only a clean tree.

So any commit not reachable from `main` kept an archived tree for 30 days, and
one dirty file kept it forever. The commits-ahead count also misleads. Of the
12, one branch's patch was already upstream, two had a net-empty diff, and four
held real unapplied plan documents.

A second gap sat under the first. `git worktree remove --force` deletes a tree
file by file. A crash midway left a half-deleted tree that `git status` reports
as hundreds of deletions, so every tier read it as dirty and kept it forever.

The user ruled that an archived thread's worktree goes, and that no work may be
lost on the way. Plan: `docs/plans/2026-10-12-archive-removes-thread-worktrees.md`.

## Decision

1. **The cleanup worker removes an archived thread's worktree.** It waits until
   the thread has been idle `ARCHIVED_REMOVAL_GRACE` (1 h, zero under hard disk
   pressure). Dirt and commits ahead of main do not stop it. This *archived
   removal* replaces Tier 0 and Tier 2 for archived threads. Non-archived
   threads keep the ladder they had.
2. **Uncommitted work is committed, not refused.** Build artifacts are stripped
   first. Remaining dirt is committed on the thread's own branch as
   `Coding agent changes (snapshot before worktree removal)`, the *snapshot
   commit*. The thread's own branch is its latest `SessionStarted.branch`.
3. **The branch stays unless it has no commits ahead** (`BranchDisposal::
   WhenMerged`). Patch-equivalence and squash detection decide nothing.
4. **Removal moves the tree aside first.** `remove_worktree_and_optionally_delete_branch`
   renames the tree to a `reclaiming-` sibling, prunes git's registration,
   settles the branch, then deletes the directory. The worker deletes any
   `reclaiming-` leftover on its next pass. Every caller gets this.
5. **A full removal wakes the branch-work net**, which sets a snapshot aside
   when no change row names the branch yet.
6. **The tree stays, with a log line saying why**, in these cases:
   - it is in use, owes a fan-in, or holds a pending change;
   - git does not confirm it is its own work tree, or HEAD is detached;
   - it holds dirt on a branch that is not the thread's own;
   - git cannot answer, or the snapshot commit fails.

## Rationale

**One owner, reading one fact.** ADR 0035 gives reclamation to the worker alone.
Archive has many emitters: the button, Archive all, the agent action, the
archive-request resolver, triage, and a parent's cascade. Every one of them
writes `archive_state`. The worker reads that state each pass, so every path is
covered, and so is an engine restart before the worker acts. No handler had to
learn to remove a tree, and no teardown reclaims one.

**A snapshot keeps work and needs no undo.** Philosophy principle 1 bars upkeep
from acting in a way the user would have to undo. The snapshot lands on the
thread's own branch, never on `main`, and nothing applies it. Moving the thread
back to Current and continuing recreates the tree from that branch, snapshot
included. The removal itself follows the user's Archive.

**Refusing would have been worse either way.** A dirty tree that blocks removal
is the reported leak. A dirty tree that blocks Archive is a new blocker with no
control to resolve it, against ADR 0378. Dirt has never blocked Archive, and the
engine already commits a dirty tree for the user at idle exit, turn end,
shutdown and Apply.

**Build output must not ride in a snapshot.** A repo that does not ignore
`target/` or `node_modules/` would otherwise get gigabytes committed onto the
branch. Stripping first uses Tier 1's prune, which skips a directory holding
tracked files.

**A kept ref is free; a deleted commit is not.** Deleting a branch the worker
believes merged by patch or by squash can be wrong. The ruling allows deletion
only for applied or discarded work. Apply's merge makes the branch an ancestor
of `main`, and Discard and Delete remove theirs.

**The rename is what makes the snapshot safe.** A snapshot of a half-deleted
tree would commit its deletions. The rename is atomic on one volume, so a crash
leaves the whole tree or a `reclaiming-` directory. It never leaves a partial
tree at the thread path.

**Never auto-archive is untouched.** `resolve_transition` exempts coding-agent
threads from the unattended auto-archive at every depth, pinned by
`cc_threads_go_to_inbox_even_when_unattended`.

## Consequences

- An archived thread's tree is gone about an hour after the archive. A later
  Move to Current and Continue rebuilds it cold, from the branch.
- The first worker pass after this ships removes trees archived long ago, under
  the same rules. No one-off script or migration runs.
- Branches of archived threads accumulate as refs until their work is applied,
  discarded or deleted. They cost no disk worth counting.
- A snapshot on a branch whose set-aside change already exists is not a new row.
  Bring back withdraws an archive-net row and decides again from the branch, so
  it picks the snapshot up then.
- Gitignored files in an archived tree are lost, as Tier 0 always did.
- A resumed agent's gap note after a full removal names the snapshot commit. It
  also says ignored files are gone.
- ADR 0035 stands. ADR 0310's "Archive is not free" now reads: the tree goes an
  hour after the archive, whatever it holds.

## Alternatives considered

- **Remove the tree in the archive handler.** Rejected. It adds a second
  reclamation owner (ADR 0035), misses the other archive emitters, and still
  needs the worker for a crash between archive and removal.
- **Refuse a dirty tree, or make dirt an archive blocker.** Rejected, see
  Rationale.
- **Delete the branch when its patches are upstream or its net diff is empty.**
  Rejected. It decides a deletion on a heuristic, and the ruling allows deletion
  only for applied or discarded work.
- **Snapshot at the archive, in the branch-work net.** Rejected as the only
  snapshot point. Dirt can appear after the archive (a background task), and
  the worker would still need its own snapshot. Two snapshotters would drift.
- **Keep the 30-day wait for archived trees ahead of main.** Rejected. It is the
  leak, and the work is safe on the branch either way.
