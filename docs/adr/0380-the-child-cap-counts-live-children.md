# 0380: The per-thread child cap counts live children, not every child ever spawned, and lives in the capacity policy

- **Status**: Accepted
- **Date**: 2026-10-06
- **Amends**: [0278: A child thread can move to top level](0278-a-child-thread-can-move-to-top-level.md)

## Context

A tester ran a coordinator thread that spawned ten sub-threads. All ten
finished, and the next `run_thread` was still refused. The agent then started
its next piece of work as a top-level thread, outside the family. The tester
went looking for a capacity setting and found Settings → System → Thread
Queue, which had nothing to do with this cap.

The recursion guard counted every child the parent had ever spawned: each row
with its `parent_thread_id`, plus each `ChildThreadDetached` on it (ADR 0278).
So the cap was a lifetime budget. Its refusal said "wait for existing children
to complete", which could never help.

## Decision

The cap limits fan-out at one moment. It counts *live children* only, and a
child that finishes frees its slot. The limit is the capacity policy field
`max_concurrent_children_per_thread`, default 10, minimum 1.

A live child is one that still owes its parent a result:

- its row is `running`, `waiting_for_user_answer` or `paused`;
- or it holds a live event wait (a *waiting child*, ADR 0254);
- or it is a spawn still queued in the Thread Queue, which has no row yet.

A child moved to top level counts while it is live. The guard joins each
`ChildThreadDetached` on the parent to that child's current row, instead of
counting the events.

## Rationale

**A finished child holds nothing.** Its turn is over and its card is written
to the parent durably (ADR 0011). Counting it only punishes a long-lived
orchestrator, and pushes the agent to route work around the family.

**Live means "the parent is still owed a result".** That is the parent's own
view of its fan-out, and it matches the states in which the parent reads
Waiting. A queued spawn is the clearest case: it will run, and a row-only count
let a parent queue any number of them.

**ADR 0278 still holds for live children.** Its point was that a move must not
buy back a slot. Joining each move to the child's status keeps that for every
child still working, and lets a finished one go like any other.

**The cap belongs in the capacity policy.** That is where the tester looked,
and `max_event_trigger_depth` already set the precedent of a limit there that
is not a pool cap. The guard and the chat system prompt read the same policy
value, so the number the agent is told is the number enforced.

## Consequences

- An orchestrator can keep spawning for as long as it runs, at most ten at a
  time by default.
- Follow-ups are still never refused. A revived child counts while it runs, so
  live children can exceed the cap through follow-ups. The cap gates spawns
  only.
- The refusal says the limit is on children running at the same time. It says
  a slot frees when a child finishes, and it never suggests a top-level thread.
- A policy stored before the field existed reads the default 10.
- The guard still checks only `run_thread`. `run_coding_agent` and
  `lucidos spawn-thread` are not gated, as before.
- One response's `run_thread` calls run one at a time, so each spawn counts
  the one before it. The cap holds without a lock.

## Alternatives considered

- **Keep the lifetime count and raise the number.** Rejected: any finite
  lifetime budget runs out for a long-lived orchestrator, and the refusal's
  advice stays false.
- **Count children whose card the parent has not yet read**
  (`parent_callback_pending`). Rejected: that ties the cap to delivery
  bookkeeping. A stuck marker would hold a slot for good, the very failure this
  removes.
- **Read `active_children_count` plus `waiting_children_count`.** Rejected:
  they cover direct children only, so they miss a moved child, and they leave
  out `paused` and queued spawns. The guard runs once per spawn, so it counts
  from ground truth in one query.
- **Keep the cap a constant.** Rejected at plan review: the user asked for it
  in the capacity policy, where the Thread Queue page shows it.
