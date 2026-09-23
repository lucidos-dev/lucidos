# 0249: A thread waiting only on its sub-threads can have its change applied: amends 0106

- **Status**: Accepted
- **Date**: 2026-09-23
- **Amends**: [0106: A parked thread's change cannot be applied](0106-a-parked-threads-change-is-not-applicable.md)

## Context

ADR 0106 withholds Apply and Discard from a thread that will wake itself. It
names two causes: a live event wait, and an active sub-thread. Its rationale is
"the change is not final, so resolving it races the work still to come".

That holds for an event wait. A thread parked on the e2e lock is still
verifying its own branch, and applying merges a branch the agent is still
working on.

For a sub-thread it is too broad. A delegating thread almost always has a
sub-thread running, so its own change was never applicable:

- The Changes panel showed only Diff, with "The thread has not finished".
- `POST /api/v1/changes/:id/apply` answered 409.
- A standing apply dropped on its first look ("The thread is waiting for a
  sub-thread.").

The cost was observed on 2026-09-22 and 23 in an app coding-agent thread that
orchestrates others. It fell back to a hand merge in the live checkout. That
merge raced a workspace script-task commit and was undone on `main`. The
orchestrator then handed every change of its own to a "carrier" child, only to
reach Apply.

## Decision

An active sub-thread no longer withholds Apply or Discard. A thread that is
idle apart from running children can have its change applied or discarded.

The gate keeps its other three causes: `Running`, `WaitingForUserAnswer`, and a
live event wait. `available_thread_actions` drops its `has_active_children`
input. Every other reader of the gate follows it:

- `unsettled_thread_ids` and the `thread_unsettled` flag (`PARKED_THREAD_SQL`),
  so the bulk paths and the per-change refusal agree.
- The standing apply, which now fires on a parent idle apart from its children.
- The status dot: a live event wait still outranks Changes to review, and a
  running sub-thread ranks below it.

Every successful apply also clears the applied branch's plan and harden
markers, in `emit_change_applied`.

## Rationale

**A sub-thread cannot race its parent's branch.** It works in its own worktree
and never writes the parent's branch. The parent's committed work is whole at
the end of each of its turns. So the thing 0106 guarded against, merging a
branch while its agent still writes to it, does not happen here.

**The case 0106 feared resolves cleanly.** After the apply, the child's
completion wakes the parent, and the parent commits again on the same branch.
The code handles that already:

1. An idle thread's apply takes Tier 2. It keeps the worktree and the branch,
   and resets the branch to the new `main`.
2. The wake resumes the same session in the same worktree, on that branch.
3. At idle, `emit_change_proposed` asks `get_pending_by_branch` for an id to
   reuse. After the apply it finds none, so it mints a new one. The only unique
   index is one pending change per branch, which the applied row does not hold.
4. The diff is taken against `main`, so only the new commits count.

So the new work comes back as a new pending change. It is not lost, not folded
into the applied change, and not refused as already merged.
`a_parents_commits_after_an_apply_come_back_as_a_new_change` proves it end to
end through the real Apply route.

**Discard moves with Apply.** 0106 withheld Discard because it "destroys work
the agent is still producing". A parent idle apart from its children produces
nothing on its own branch. Discard resets that branch to `main` and keeps the
worktree, exactly as it does for a settled thread the user later follows up.
The next wake resumes there, and the turn-gap note tells the agent its work was
discarded. The reason does not hold, so the pair keeps moving together.

**The markers had to move to the shared emit.** The branch outlives the apply,
and a stale harden marker still counts as hardened. Only Apply Now cleared the
markers. So after a Tier-2 apply, the parent's next change rode the previous
change's plan approval and `/harden` run. That was already true for any
follow-up after an apply, and the narrowed gate makes it the ordinary path for
a Lucidos-source delegating thread.

`emit_change_applied` is the one emit every merge path performs once, so the
clear lives there. It is gated on the accepted emit, so a suppressed duplicate
cannot wipe a marker recorded for newer work.

## Consequences

**Kept.** Everything 0106 decided for an event wait: the gate, the dot
precedence, the Stop waiting exit, and the accepted gap between a delivery and
its wake. `is_blocking`, `is_attention_needing` and `display_section` are
untouched. A parent with running children still sits in Current.

**The two waiting causes now differ in what they gate.** 0106 took both into the
gate partly to keep the one Waiting dot honest. The dot now follows the gate
instead. A parent with a change and a running child reads Changes to review,
and one with no change still reads Waiting. The waiting indicator still lists
the running sub-threads either way.

**A Lucidos-source thread plans and hardens again after an apply.** This is what
Apply Now already did, and what its comment said every path should do. App and
external-repo changes skip both gates, so an app coding agent is unaffected.

**A narrow window is accepted, not closed.** A child can finish while its
parent's change is mid-apply. The wake then starts a turn in the worktree the
apply is resetting. A user already has the same window by typing a follow-up
straight after pressing Apply. The Tier-2 fast path runs for seconds, and a
resumed agent needs longer to boot, edit and commit.

Closing the window needs a per-thread apply lock that the spawn path honours.
That is a larger change than the risk warrants today, in the same spirit as
0106's own accepted window.

**Contract.** `available_thread_actions` lost a parameter, so the generated
TypeScript and the cross-validation fixture were regenerated. The fixture's
cross product shrank by one boolean dimension.

## Alternatives considered

**Keep the gate and teach the orchestrator a carrier child.** That is what the
orchestrator did on its own. Rejected: it costs a child thread per change, and
it leaves every delegating thread with an unusable Apply button. The rule it
works around guards a race that does not exist for a sub-thread.

**Gate on a sub-thread only when it shares the parent's worktree.** No spawn
path gives a child its parent's worktree, so the input would always read false.
A fact that can never be true is dead code in the predicate and its contract.

**Allow Apply but keep Discard withheld.** Rejected for the reason 0106 gave in
reverse. Discard's reason for being withheld was the same "work still being
produced", and that reason is gone. Splitting the pair would leave a parent
with an Apply button and no way to throw the change away.

**Leave the stale markers for a separate change.** It was offered as a fork, and
turned down. The narrowed gate makes "apply, then keep working on the branch"
the ordinary path for a delegating thread. Shipping that without the clear
would make the gate bypass routine.
