# 0463: A landing lane is the only mover of main for Lucidos-source changes

- **Status**: Accepted, amended by [0480](0480-apply-all-press-lands-as-one-lane-batch.md)
- **Date**: 2026-10-12
- **Amends**: [0060](0060-apply-merge-ownership.md),
  [0241](0241-session-spawn-does-not-merge-main.md),
  [0314](0314-apply-all-parks-a-conflicting-change.md),
  [0442](0442-a-check-and-its-claim-are-one-step.md)

## Context

Parallel coding-agent threads in the Lucidos repo lost hours to merge
conflicts at Apply. Each conflicting change got its own resolver, resolvers
overlapped, and five resolutions in three weeks failed because `main` moved
under them. One Apply All of 13 changes spawned 5 resolvers, three on one file.
The measurements and the grill are in `docs/plans/2026-10-12-landing-lane.md`.

## Decision

One engine-owned **landing lane** moves `main` for Lucidos-source changes.
Every Apply surface queues into it: Apply, Apply Now, Apply All, a standing
apply, the LLM `apply_change` tool, auto-apply after hardening and chat
auto-apply.

- **Apply queues a snapshot.** `apply_change` keeps every gate up to and
  including the harden backstop, the branch check and the clean-repo check.
  It then commits a dirty worktree, as Tier 2 did, and queues the branch head
  as the change's *snapshot* (`ChangeQueued`). A queued change is frozen. A
  second Apply keeps the snapshot it has.
- **Clean work lands at once.** A pass builds one *landing merge* per member
  with `git merge-tree --write-tree` and `git commit-tree`, chaining the tip
  forward. It holds the merge lock for the whole pass and moves `main` with
  the guarded `ff_main_to_locked`. Each member's `ChangeApplied` carries
  `landing_merge_sha`, under its apply claim. A batch the pass opens starts
  on `main` as the pass left it, its own landings included.
- **Every mover of `main` takes the merge lock first**, then the repo lock
  (`MainLocks::lock_main`). Revert and the safe-file commit of the clean-repo
  check move `main` too, so they wait out a pass rather than fail its guarded
  move.
- **What would conflict forms one lane batch.** At most one resolves at a
  time (`LaneBatchStarted`). A later arrival whose files share no path with
  the batch's files lands at once. An overlapping one waits and starts the
  next pass when the batch ends (`LaneBatchLanded`, `LaneBatchFailed`).
  *Amended by ADR 0480: every arrival gets a clean pass, and the lane
  gathers while an Apply All press is open.*
- **A lane thread resolves the batch.** As the batch opens, the lane creates
  its *integration branch* `lane/<batch id>` at the base, and the lane
  worktree `lane-<short id>`. It opens one coding-agent thread with a fresh id
  recorded on `LaneBatchStarted`, titled for what it lands. Its first prompt
  merges each member with `git merge --no-ff <snapshot>`, in order, as its own
  landing merge. Then it resolves inside each merge, runs `/harden` once, and
  asks the maintainer when a resolution needs a judgment. It never rebases and
  never touches a source thread's worktree.
- **Any run on the lane thread carries the batch.** `run_direct_agent` asks
  whether its thread is an open batch's lane thread. If so, the run works in
  the lane worktree and proposes nothing. It ends at the first idle that owes
  no follow-up, the rule every session exits by. So a message the maintainer
  sent mid-turn gets its turn before the batch lands. The first turn, a
  follow-up the maintainer types, a catch-up turn, a Continue and a switch
  resume all carry it alike.
- **A closed batch's branch goes before the thread runs again.** A run on the
  lane thread first removes the integration branch the lane left. It is
  refused only while that removal fails, and each later run tries again.
- **Every run carrying a batch answers the lane once.** Its completion
  reports how the turn ended. A run that never reaches it answers from
  `run_direct_agent`: a Stop during startup keeps the batch open, and a run
  that cannot start or prepare the lane worktree fails it. So does a lane
  turn the router could not spawn. A shutdown and a stale resume answer
  nothing: the restart and the caller's retry answer instead.
- **A Stop with no live run answers the lane too.** It never proposes the
  integration branch. That holds while the batch is open, or while the
  thread's last session ran on the integration branch. A former lane thread
  that has since run on its own branch gets the ordinary teardown.
- **A clean finish lands the batch whole.** The lane waits out the run's
  teardown and holds the thread. It checks the branch: no merge in progress,
  no unmerged or uncommitted path, and exactly one landing merge per member
  with its snapshot as second parent. Under the merge and repo locks it
  merges `main` in when `main` moved, then fast-forwards `main`. Each member's
  `ChangeApplied` names its own landing merge, and `LaneBatchLanded` follows
  with `main` before and after.
- **A catch-up conflict is a turn, never a failure.** Say `main` gained work
  that conflicts with the resolution. The catch-up merge is aborted, and the
  same lane thread gets a follow-up turn naming what `main` gained. Its next
  clean finish lands the batch.
- **A Stop pauses the resolution, it does not end it.** As on any thread,
  Stop ends the lane thread's turn and keeps the session resumable. The batch
  stays open with its worktree, reads as resolving, and its overlap rules
  hold. The thread's next turn, a follow-up or a Continue, carries it on.
- **A failed batch fails whole and queues nothing.** These fail it: a failed
  turn, a run that cannot start, and a branch that fails the check. So do
  the lane row's Cancel (`POST /api/v1/changes/lane/cancel`), and
  discarding, archiving or deleting the lane thread. Every member stays
  pending and apply-ready, each with `ChangeUnqueued { reason: batch_failed }`.
  The lane removes the lane worktree and branch once the lane thread holds no
  run. Cancel takes the actor check, and the reach check over every member,
  as Apply does.
- **An archive or a delete fails the batch, live run or not.** The lane hears
  `ThreadArchived` and `ThreadsDeleted` on the bus and ends the batch as
  Cancel does. An idle lane thread, the usual state after a Stop, has no run
  whose end would report it. A restart reads an archive or delete the lane
  missed, and fails the batch rather than open the thread again.
- **The source thread stays free.** After landing, its worktree resets to
  `main` in one case only. The branch still sits at the snapshot, the tree is
  clean, and no run holds the thread. Until the change lands or fails, its
  turn ends propose nothing new. The work is withheld as `awaiting_landing`,
  then decided again once.
- **Revert undoes one change.** A change with a landing merge reverts exactly
  that commit with `git revert -m 1`. Older rows keep the earlier revert.
- **The lane keeps no table.** A restart folds the lane's events and its
  members' `ChangeApplied` into the queue and the open batch, with its lane
  thread. A batch whose lane thread was never prompted opens it. One whose
  lane thread finished cleanly lands. A cut-off lane thread is left to agent
  recovery: a user switch resumes it, a crash leaves it a Continue, and no
  second batch opens meanwhile. A landing a crash cut off heals from `main`:
  a queued snapshot that is the second parent of a merge on `main`'s
  first-parent line is announced as landed.
- **The lane acts only on its restored state.** It handles no message until
  the restore arrives, so no Apply meets an empty lane and no restore drops a
  claim a pass took. A restart retires the lane worktrees of closed batches
  this workspace opened, and no others: several workspaces can share one
  checkout.
- **A waiting change resolved elsewhere leaves with its reason.** A pass
  that finds a waiting member set aside or discarded drops it with
  `ChangeUnqueued`, so no restart queues it again. The path that resolved it
  owns its thread's work: the lane settling it would bring a set-aside change
  back (ADR 0328). Every member a pass lands, or fails to land, settles its
  thread.

App changes keep the Apply tiers. External-repo threads still propose nothing.

## Consequences

- A clean Apply still lands in seconds, now as one merge commit per change.
- An Apply can answer `Queued`. Every surface reports it as such, and
  `ChangeApplied` or `ChangeUnqueued` follows.
- Nothing but the lane and the app path moves `main`.
  `only_the_lane_and_the_app_path_move_main` names each caller. No
  Lucidos-source change can fail with the concurrent-main error.
- A pass-through that lands while a batch resolves sits behind the batch's
  catch-up merge on `main`'s first-parent line. Revert still undoes exactly
  it, but `git log --first-parent main` shows the catch-up there instead.
- The engine writes the integration branch a simple-fix plan marker, because
  every member cleared its own plan floor at Apply. Without it, the plan gate
  would block the lane thread's first edit.
- The batch's files are what its integration branch changed since the base,
  plus each member's own files. The members count so an arrival in the window
  before the lane thread merges them still waits.
- A waiting change can be taken back out with Remove
  (`DELETE /api/v1/changes/:id/queue`, `ChangeUnqueued { reason: removed }`).
  A member of the open batch cannot: it leaves only with its batch. Remove
  takes the actor and reach checks, but not the thread-state gate. The source
  thread stays free while its change waits, and Remove touches no branch.

### Amendments

- **ADR 0060.** Ownership of a Lucidos-source change's merge passes to the
  lane. The `MergeConflictDetected` pairing and the resolver binding guard
  app changes only. A lane thread binds its session to the batch
  (`ConflictBinding::LaneBatch`), never to one change.
- **ADR 0241.** "Only Apply may merge `main` into a thread's branch" now reads
  "nothing in the engine does, for Lucidos source". The lane merges `main`
  into its own integration branch only, never into a thread's worktree.
- **ADR 0314.** Apply All stays the orchestrator of a press, which keeps one
  rule for app and Lucidos members alike. A member the lane answers `Queued`
  parks like a conflict, and never earns the one retry: a failed lane batch
  queues nothing by itself. A failed batch reports each member's failure to
  Apply All directly. App members keep the retry.
- **ADR 0314, Cancel.** Apply All's Cancel reaches the lane: a waiting member
  leaves it (`ChangeUnqueued { reason: removed }`), and a lane batch holding a
  member fails whole. It never interrupts a lane member's source thread,
  whose agent no longer carries that change.
- **ADR 0442.** The lane holds a member's apply claim from the moment its
  pass starts until it lands or its batch fails. The claim stays with the
  lane, not with a lane-thread run, so a watchdog continuation needs no claim
  hand-off: the continuation inherits the batch, and the wedged run's end
  reports nothing. A waiting member holds none, so Discard and Set aside take
  it, with `ChangeUnqueued`. Each surface takes the claim before anything
  else, so a refused Discard has not yet stopped the source thread's
  background tasks.

## Rationale

- **One mover cannot race itself.** Every failure because `main` moved came
  from two movers. With one, a pass reads `main`, builds, and moves it under
  one lock.
- **Exact footprints make pass-through safe.** Disjoint file sets cannot
  conflict textually, so clean work never waits behind a resolution. An
  unknown git answer counts as overlap, the direction that only costs a wait.
  *Retired by ADR 0480: the merge itself is the test.*
- **A snapshot frees the thread.** The agent can keep working while its
  change waits, and nothing it commits later lands by surprise.
- **One landing merge per change** keeps Revert exact, and makes
  `git log --first-parent main` one line per landed change.
- **One resolver per batch.** The lane thread sees every conflicting change
  at once. Two changes competing for one file get one resolution, and a
  judgment between them goes to the maintainer.
- **The thread knows its duty.** The batch binds to the thread, not to one
  run. So a follow-up, a Continue and a switch resume carry it without a
  second spawn path.
- **Events, not a table.** The queue is small and its history is already in
  the events, so a projection would only add a second source to keep in step.

## Alternatives considered

- **Drain every Apply All member into one pass.** It lands a press faster, but
  splits the press into two orchestrators, one per kind of change. Keeping the
  Apply All driver costs one pass per member, which is seconds each.
  *ADR 0480 keeps this rejected, and gets one batch per press by gathering.*
- **Answer the caller only once a change lands.** A resolving batch can take
  many minutes, which is why every tier already detaches. The lane answers
  `Queued` and reports the landing as `ChangeApplied`.
- **A lane worktree for the clean pass.** `merge-tree` and `commit-tree` need
  none, and a worktree would add state to clean up after a crash.
- **Take each member's claim after `main` moves.** A held claim then strands a
  member already on `main`. The lane claims before it builds, and skips a
  member whose claim another holds.
- **Lend each member to the Tier-3 merge session.** Phase 1 did, as an
  interim. Members landed one at a time from their branch heads, so a batch
  could land in part and lose its snapshots. Retired with the lane thread.
- **Let the lane-thread run carry the claims, as Tier 3 does.** The batch
  outlives any one run: a catch-up turn and a Continue are new runs, and the
  landing itself follows the run. Handing claims from run to run, and back to
  the lane to land, buys nothing the lane holding them does not.
- **Rebuild the landing merges on the new `main` instead of a catch-up
  merge.** It would keep pass-throughs on the first-parent line, but replaying
  a resolution means replaying merges, which git cannot do faithfully.
- **Send a follow-up turn when the completion check fails.** An agent that
  ends its turn without its merges has decided something. A loop of follow-ups
  would retry with no end condition. The batch fails, names what was missing,
  and the maintainer presses Apply again.
- **Fail the batch on a Stop.** It deleted the whole resolution, worktree and
  branch, where Stop everywhere else keeps the session. The lane row's Cancel
  is the one deliberate way to end a batch from outside.
- **Retire a closed batch's branch only from the lane's cleanup task.** A
  removal that timed out then refused the lane thread until the next
  restart. The run that holds the thread's claim is the one place nothing
  else works in the lane worktree, so it retires the branch itself.
- **Read every early exit of a lane run as a Stop.** It kept the batch open,
  but a run that could not start or prepare its worktree has nothing to
  resume. Those fail the batch whole, the lost claim included, and the
  maintainer presses Apply again.
- **Refuse to archive or delete a lane thread with an open batch.** The
  maintainer would then have to Cancel first, from another surface, to put a
  thread away. Failing the batch is what Cancel would do anyway.
- **Hand a stale Stop to the lane only while the batch is open.** After the
  batch closes, the thread's last session can still be the lane's: the
  integration branch outlives a removal that failed. The ordinary teardown
  would then propose that branch as a change. So the session's branch decides.
- **A `withdrawn` unqueue reason.** A withdrawn change never returns under
  its id. Neither does a discarded one. So it leaves the lane as `discarded`,
  and the wire keeps its five reasons.
- **A second spawn path for the lane thread.** It would duplicate the run's
  question flow, steps, recovery and Continue. The router's ordinary engine
  turn, plus the duty lookup in `run_direct_agent`, reuses all of them.
