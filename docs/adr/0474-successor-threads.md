# 0474: A heavy coding-agent thread continues in a successor thread that carries its branch, sits in its place, and proposes for both

- **Status**: Superseded by [ADR 0560](0560-successor-session-in-the-same-thread.md): a handoff starts a successor session in the same thread. Old successor-thread chains still render.
- **Date**: 2026-10-14

## Context

Long coding-agent threads pay for their whole context on every call. In the
dev workspace, threads past 500k tokens carried 61% of Claude Code spend. Work
after a plan approval ran at an average 300k in the planning thread itself.

Before this, a session could only hand off by hand: write a file, copy a
resume prompt, run `/clear`. None of that is visible to Lucidos: no link
between the two halves, no branch carried, and a parent waiting on the old
thread is never told.

Plan: `docs/plans/2026-10-13-auto-compact-trial-and-successor-threads.md`.

## Decision

A thread continues in a *successor thread*, through one route
(`POST /api/v1/threads/:id/successor`) and one CLI (`lucidos successor spawn`).

- **The successor's branch starts at the old thread's last commit.** Its change
  carries all the work. The old thread's work is held as
  `carried_by_successor` inside `read_proposal_hold`, so no boot sweep or later
  turn end proposes it.
- **It sits in the old thread's place.** Top-level if the old thread was,
  otherwise a child of the same parent with the same wake mode. The old thread
  files no report: its successor reports instead.
- **The links are one event**, `SuccessorThreadSpawned` on the old thread,
  projected to `successor_thread_id` there and `successor_of` on the successor.
- **The old thread archives itself** through the archive request once the
  successor's first session starts, so a blocker leaves it open.
- **The successor's branch gets a copy of the old branch's plan marker.** It
  carries the same work, so the user's decision on it carries too.

## Rationale

**Carry the branch, never merge two.** A successor branched from `main` would
split the work across two changes, and either Apply could land half of it.
Starting at the old head makes the successor's change the only one. An
unreadable base fails the spawn rather than fall back to `main`.

**Hold inside the decision, not at the moment.** The boot sweep re-derives
every unproposed thread's reason from `read_proposal_hold`. A hold written
once at handoff would be re-proposed at the next boot.

**Unknown keeps the work.** The archive net skips the old thread's branch only
when that work is known to be carried. Either its head is still the handoff
base, or git confirms the head is in the successor's branch. Any unknown answer
sets the work aside, which keeps it. Commits the old thread makes after the
handoff are set aside once the successor's change is applied or discarded.

**The place, not a detach.** A successor spawned in place keeps the old
thread's parent and wake mode in one step, so a waiting parent waits on it. A
top-level successor is a top-thread, which needs no owner approval (ADR 0475).
A child one acts in its parent's subtree, so from a turn the owner did not open
it needs `spawn-successor` on the parent.

**Copy the marker.** Without it, every mid-work successor's first proposal
waits for a plan marker, and the agent asks the user to approve a plan they
already approved.

## Consequences

- `UnproposedReason` and `ProposalHold` gain `CarriedBySuccessor`, and the
  database CHECK gains its value.
- `ThreadReachVerb` gains `SpawnSuccessor`.
- `EngineReason` gains `SuccessorResidue` for work set aside after a handoff.
- A dirty worktree refuses the handoff, so an agent commits first.
- An integrated sub-thread never hands off: its parent's agent merges its
  branch.
- The family root follows `successor_of`, so a successor shares its old
  thread's auto-compact trial arm.

## Alternatives considered

- **Invert `IntegratedIntoParent`** (the old thread as an integrated child of
  the successor). It carries the fast harden tier, the hand-up and the parent
  copy, none of which fit a predecessor.
- **Let the successor merge the old branch in its first turn.** A failed merge
  would leave a successor already running with half the work. Branching at the
  old head cannot fail that way.
- **Keep the manual flow** (a handoff file, a resume prompt, `/clear`). It
  cannot link the threads, carry the branch or move a parent's wait, and it
  lives outside every workspace's history.
- **Guard a second handoff with an in-process claim set.** It does not span
  two engines during a Switch. A Postgres advisory lock with a re-read
  serializes the route, and the projection refuses a second link as the
  backstop.
- **Retry taking back the old work after the link.** `record_handoff` reads
  everything before the link, so only a withdraw can fail after it. The
  proposal hold already keeps that work from being proposed again.
