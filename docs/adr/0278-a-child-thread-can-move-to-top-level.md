# 0278: A child thread can move to top level: the edge is cut by an event on the former parent, and nothing is stopped

- **Status**: Accepted
- **Date**: 2026-09-24
- **Amends**: [0011: A blocking child's completion is durably delivered to its parent](0011-parent-child-fan-in-durability.md)

## Context

A child thread (spawned by `run_thread` or `run_coding_agent` with
`relation: "child"`) stayed nested under its parent for life. The only way out
was to stop it and respawn it at top level, which lost its work in progress.
Hand-editing `thread_summaries.parent_thread_id` is not an option: events are
the authority, and the projection must be rebuildable from them.

The edge carries five meanings at once. It is the counter membership, the wake
contract, the archive cascade, the fan-out cap and the follow-up authorization
edge (ADR 0011, ADR 0043). Cutting it changes all five together.

## Decision

One persisted event, `ChildThreadDetached { child_thread_id,
child_thread_title }`, recorded on the **former parent**. Its projection arm
cuts the edge on the child's row: `parent_thread_id` becomes NULL, the owed
callback and stopped-child flags clear, and `depth` is rebased over the whole
subtree. The former parent and its ancestors are recounted from ground truth.

The move never stops the child. A turn in flight finishes, keeps its work and
proposes its change, and its result lands on its own timeline only.

Two callers, told apart by what the engine verified:

- **An agent** moves only its own direct children, the ladder of ADR 0043. It
  reaches the move through the `threads` tool's `detach_child` action, or the
  route and `lucidos threads detach` with its origin token.
- **The user** moves any thread that has a parent, from the thread menu's
  **Move to top level**, through `POST /api/v1/threads/:thread_id/detach` with
  no origin token.

## Rationale

**The event lands on the parent, not the child.** ADR 0011's two recovery
checks ask whether a thread's LATEST event is a `ChildThreadCompleted`. An
event on the child would become its latest event. A child that is itself a
parent would then lose a grandchild's unprocessed card, and its worktree with
it. On the parent, the event also sits where the counters move. It is what the
parent's agent reads next turn, so it stops waiting instead of respawning.

**Late deliveries are dropped at the bus, in the transaction.** The fan-in
reads the edge once, then acts over several round trips, so a move can land in
between. The emit re-reads the child's parent under a share lock and drops a
`ChildThreadCompleted` or `ChildThreadStopped` for a child that moved out. The
same check drops a second move of the same child. Because no card is written
after a move, every card the boot sweep finds was earned before it, and is
still delivered.

**A move frees no child slot.** The fan-out cap counts the parent's children
plus its `ChildThreadDetached` events. Otherwise a move is a repeatable way
round the cap of ten.

**A moved coding-agent child asks a human.** `resolve_attend_mode` walks the
spawn event's `parent_thread_id`, which events never rewrite. The walk now
stops at a moved thread and answers `Interactive`, so a thread the user cut
loose cannot keep a trigger's side-effect grant. An unreadable answer counts
as moved, which fails closed.

**The user-facing word is "Move to top level", not "Detach".** *Detached*
already means something else here (ADR 0049, every event wait is detached).
Internal identifiers keep "detach".

## Consequences

- The recovery checks of ADR 0011 skip a trailing `ChildThreadDetached`, as
  they skip a `ChildThreadStopped`, so moving one child cannot strand a
  sibling's unprocessed card.
- The boot child-count rebuild now covers `total_children_count`, and resets a
  former parent left with no children.
- A move cannot be undone. There is no re-attach.
- A parent that armed its own `await_event` on the moved child's
  `ChildThreadCompleted` is not told. That wait runs until its timeout, at most
  24 hours. Cancelling it needs a new `EventWaitCancelCause`, which the
  subscription work below would have brought.

## Alternatives considered

- **The event on the child (`ThreadDetachedFromParent`).** Rejected for the
  recovery-check reason above.
- **Rewrite the fan-in wake as an engine-armed thread subscription**, so that
  "stop waiting" cancels one. Rejected: once "stop waiting" means cutting the
  edge there is nothing left to cancel, since the fan-in already ignores a
  thread with no parent. The rewrite would also have needed exemptions from
  the live-wait cap, the subscription cap, the 24-hour ceiling and ADR 0052's
  archive rule.
- **Let the agent reach any descendant, as the thread-reach ladder does.**
  Rejected: the user asked that an agent move only its own direct children,
  which is the edge ADR 0043 already names.
- **Refuse a move once the child's card is on the parent.** Rejected. The card
  was earned before the move, so delivering it is correct. The refusal would
  also land on the user in a race window they cannot see.
