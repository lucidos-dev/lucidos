# 0254: A child that ends its turn holding an event wait sends its parent no callback: the turn the wait wakes reports instead

- **Status**: Accepted
- **Date**: 2026-09-23

## Context

An orchestrating coding-agent thread ran several *child threads*. Two of them
armed an *event wait* for a shared bench slot and ended their turns, as the
event-wait guidance tells them to. Each turn's `CodingAgentIdled` reached the
parent as `ChildThreadCompleted: success`. Both children were still working:
each resumed when its wait fired, and each later finished for real.

The parent read two finished children and started planning the next step on
work that had not run yet. It is the same false completion ADR 0252 closed for
a user Stop, reached by another road.

## Decision

A child whose turn ends while it holds a live event wait sends its parent no
card. The parent gets nothing, not even a note. `parent_callback_pending` stays
set, so the turn the wait wakes reports when it ends.

Two acts end a wait without waking the child, and each still owes the parent:

- **The wait ends by Stop waiting, or the agent stands it down.** When no wait
  is left and the child is idle, the parent gets the card that was held back,
  with the child's real status.
- **The user archives the child.** The parent gets a canceled card, exactly as
  for an archived *stopped child*. A Discard does not settle a child that still
  holds a wait, because that child wakes again.

A failed turn still reports at once, wait or not.

## Rationale

A subscription does not hold a turn (ADR 0049), which is why waiting threads
read as idle. But idle is not done. The fan-in treated the end of every turn as
the end of the child, and a child that waits ends several turns before it
finishes.

No note on the parent, unlike a stopped child. A stopped child needs the user,
so the parent has something worth knowing. A waiting child needs nothing from
anyone: the engine wakes it. A note would only invite the parent to act.

## Consequences

- A parent waiting on a child that waits sees nothing until the child's last
  turn ends. That is slower to hear, and it is true.
- A wait delivered before the turn that armed it has idled no longer counts
  as live at the idle. That turn reports, and the woken turn reports again.
  This is the double card every re-woken child already sent, and it is rare.
- `settle_child` settles any child still owed a card and not in flight, on
  Archive, Discard or Delete. That also covers a child a crash left owed and
  nothing resumed.

## Alternatives considered

- **Send a "waiting" status that does not wake.** Rejected for the reason a
  stopped child's note has no status: a parent with nothing to do should not be
  handed something to read as a result.
- **Count a waiting child as active on the parent.** Rejected. The active
  count means "running", and ADR 0049 keeps a subscription out of it.
- **Stop children from arming waits at the end of a turn.** Rejected. Arming
  a wait and finishing the turn is the event-wait contract, and the guidance
  tells agents to do exactly that.
