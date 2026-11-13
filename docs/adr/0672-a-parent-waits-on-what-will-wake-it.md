# 0672: A parent waits on what will wake it

- **Status**: Accepted
- **Date**: 2026-11-13
- **Amends**: [0254](0254-a-child-holding-an-event-wait-has-not-finished.md)

## Context

A top thread showed no waiting dot while its family still worked. Its child
(wake mode `wake`) was idle and had already reported. That child waited on a
running grandchild (also `wake`).

The dot read `active_children_count + waiting_children_count`, and both count
direct children only. That was decision D2 of
`docs/plans/2026-09-24-a-parent-waits-on-a-child-that-waits.md`. D2 assumed a
child that has reported owes its parent nothing more.

It does. When the grandchild finishes, it starts a new turn on the child. Every
new turn re-arms `parent_callback_pending`, so the top thread is owed a second
report and wakes on it.

The same review found the reverse. A running `no-wake` child painted its parent
as waiting, though nothing will wake the parent (ADR 0451, 0545).

Plan: `docs/plans/2026-11-13-a-parent-waits-on-what-will-wake-it.md`.

## Decision

A parent waits on an *awaited child*: a direct child for which both hold.

- **Its finish wakes the parent.** Its `wake_mode` is `wake`,
  `wake-when-all-done`, or NULL. A NULL child predates wake modes and decides
  per turn, so it may wake.
- **It is unfinished.** Its status is active, it holds a live event wait, or it
  awaits a child of its own.

The last clause is the same rule one level down, so the roll-up climbs any
depth. A `no-wake` child is never awaited, and it stops the climb above it.

The engine projects the rule as a per-thread `awaited_by_parent` flag and the
parent's `awaited_children_count`. One SQL predicate defines both. The drawer
dot, the waiting indicator and the turn-end read decision read them.
`waiting_children_count` retires.

## Rationale

The waiting dot promises one thing: something will wake this thread. The wake
path decides what counts, not the shape of the tree. A grandchild's finish
reaches the top thread through the child's next turn, so the top thread waits
on it. A `no-wake` child files its reports and wakes nobody, so its parent does
not wait on it.

A per-row flag lets a client list exactly the children the count counts. The
count decides and the list only names them, so the two cannot disagree.

## Consequences

- A parent over any `wake` chain with work still running shows waiting, at any
  depth.
- A running `no-wake` child leaves its parent's waiting indicator. The drawer
  still nests it under the parent with its own spinner.
- Every change to a child's status, waits, or awaited count recounts each
  ancestor in the same transaction, child before parent.
- `active_children_count` keeps its meaning. It means "running", and the child
  cap (ADR 0380), sections, worktree cleanup and fan-in (ADR 0011) read it. A
  running `no-wake` child still holds a child slot.

## Alternatives considered

- **Keep D2 and count direct children only.** Rejected. It hides a wait the
  engine will deliver, which is the reported bug.
- **Count every unfinished descendant.** Rejected. A `no-wake` descendant never
  reaches the parent, so the dot would promise a wake that never comes.
- **Derive the roll-up in each client.** Rejected. The drawer does not load
  every descendant, and two derivations drift. The engine holds the ground
  truth.
- **Widen `active_children_count`.** Rejected for the reason ADR 0380 gives:
  it means running, and its readers rely on that.
