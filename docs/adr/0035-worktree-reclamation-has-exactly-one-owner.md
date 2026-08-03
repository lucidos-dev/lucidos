# 0035: Worktree reclamation has exactly one owner, the cleanup worker, never a session teardown

- **Status**: Accepted
- **Date**: 2026-08-03

## Context

On 2026-08-03 the coding-agent completion path destroyed two live worktrees in
one morning, taking a user's uncommitted work with it both times. The line was
this, in `agent_session/run_session/completion.rs`, running with no
preconditions at session teardown:

```rust
// Remove the worktree directory (the branch stays)
git_cmd(&["worktree", "remove", "--force", &wt_path_str], &repo_root).await
```

It only logged on *error*, so a successful wipe was completely silent. That is
why the first investigation of the engine log found no removal anywhere and
concluded the directory had vanished on its own.

Two independent routes reached it.

**The probe route.** With the host saturated by a full Playwright run, ordinary
`git rev-parse` calls were blowing `git_cmd`'s 30s ceiling. Two predicates read
those timeouts as facts, "the branch is gone" and "the worktree is stranded",
the spawn path started over on a fresh branch, and the teardown then force-removed
a live tree. Fixed separately by making an unanswered probe `Unknown` rather than
a `no` (see `.claude/rules/rust.md` and
`docs/plans/2026-08-03-unknown-git-state-must-not-delete-worktrees.md`).

**The race route.** A session died mid-turn to a SIGKILL. The safety net did its
job and relaunched Claude Code into the same worktree within one second. The
*dead* session's teardown was still unwinding, reached the removal, and deleted
the directory out from under the process that had just started in it. The engine
log shows the interleave in four lines:

```
07:37:59  [run] CC process spawned: 274.613375ms
07:37:59  [run] Injecting resume note … for thread 31ae0c44
07:38:00  [completion] Safety net fired with commits … keeping branch on disk
07:38:00  [run] Result event received … error: Path "…/thread-31ae0c44" does not exist
```

Same shape again at 07:44:43. The newly spawned session's own SessionEnd hook
failed with `posix_spawn '/bin/sh': ENOENT`, the signature of a deleted working
directory.

A positive-evidence gate closed both routes. This ADR records the decision made
after it landed: **the call site should not exist at all.**

Three things settle it.

**It was a second owner of an operation that already had one.** `WorktreeCleanup`
is the designated reclaimer and is far more careful: Tier 0 removes only
zero-information trees (clean, no pending change, no commits ahead) after a 1h
grace, Tier 1 strips regenerable build artifacts at 24h idle, Tier 2 does full
removal at 30d, disk pressure escalates all of it, and fan-in retention exempts
threads a child still depends on. It sweeps every 15 minutes and it is what
actually holds disk in check. The teardown removal reclaimed the same bytes at
most one hour earlier.

**It reclaimed almost nothing.** A clean turn does not reach the completion
stage at all: the idle exit returns earlier, in `run_session/run.rs` ("CC process
exited while idle"). Confirmed against a live workspace, where thread worktrees
are created once and then logged as "Reusing existing worktree" on every later
turn. So the removal only ever ran when a session ended *abnormally*.

**It fired in the worst possible place.** A teardown frequently runs precisely
because something went wrong, which is when the engine's picture of the world is
least reliable, and it can be unwinding concurrently with the safety net
relaunching a session into the very tree it is deleting. Two subsystems with
opposite intentions aimed at one directory, with no ordering between them.

The cost asymmetry is total. Keeping a worktree costs reclaimable disk for up to
an hour. Deleting one wrongly costs the user work that cannot be recovered.

## Decision

**Worktree reclamation has exactly one owner: the background `WorktreeCleanup`
worker.** No session teardown path reclaims a worktree.

The session path keeps exactly one removal, and it is not reclamation: an
explicit user **Discard**, where the user has asked for the work to go away and
the branch is deleted in the same operation. It keeps one guard, that the tree is
checked out on the session's own branch, because Claude Code can `git checkout`
inside its own worktree and a Discard must not delete a tree that is no longer
the one it names. An unreadable or detached branch is not a positive match.

`session_worktree_removal_decision` collapsed to `discarded_worktree_removal`
accordingly. Its liveness and dirtiness arms went with the call site that read
them. `worktree_liveness`, `worktree_dirtiness`, `GitAnswer` and `or_unknown`
all remain for their other callers.

Unchanged and still correct, because neither is reclamation:

- The conflict-resolution abort removes the **Tier-3 temp worktree the merge
  attempt itself created**, gated by `conflict_abort_deletes_temp_state`. That is
  the failure-path-cleanup rule working as intended: delete only what this
  attempt created.
- `agent_recovery`'s discard path, which is the same category as the Discard
  above.

## Consequences

- **Worktrees linger longer.** A finished thread keeps its tree until Tier 0
  reclaims it after Apply (1h grace, 15min sweep), or Tier 1 / Tier 2 on idle. The
  steady-state count on disk rises. This is the intended trade, not a regression
  to fix by re-adding a teardown removal. If the count becomes a real problem, the
  answer is a measured change to the worker's tiers, in one place, with the
  worker's evidence.
- **Startup recovery already absorbs this.** It short-circuits on a clean tree
  with no in-flight signal ("Skipping clean worktree … cleanup worker will
  reclaim"), so the extra trees hit the cheap arm and the 30s API readiness budget
  holds.
- **The property is enforced by a test that reads the source**, not just by
  behaviour: `the_completion_path_removes_only_the_two_worktrees_it_is_allowed_to`
  in `agent_session/lifecycle_tests/worktree_removal.rs`. A behavioural test can
  only show that today's reachable paths keep the tree; it cannot stop the next
  reader from adding a removal back onto the abort path, which is exactly what
  happened twice in one morning. The test names both legitimate removals and
  fails on a third.
- **Do not re-litigate this by pointing at disk.** Disk was the original
  motivation and it did not survive contact with the numbers: the removal never
  ran on the clean path that produces most worktrees, and the worker reclaims the
  same bytes within the hour.
