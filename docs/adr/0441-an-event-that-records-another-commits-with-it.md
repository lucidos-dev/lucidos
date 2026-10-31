# 0441: An event that records another commits in the same transaction, through an atomic emit

- **Status**: Accepted
- **Date**: 2026-10-12
- **Amends**: [0047: An event wait is an event](0047-event-wait-is-an-event.md), [0256: An agent message waits behind an open question](0256-an-agent-message-waits-behind-an-open-question.md), [0266: A standing apply waits through an event wait](0266-a-standing-apply-waits-through-an-event-wait.md)

## Context

Two places wrote a marker in one emit and the thing it records in a second:

- **A held message.** `HeldMessageReleased`, then the `MessageReceived` that
  delivers it. If the second emit failed, the message was neither held nor
  received. "Held" means "no release names it", so nothing read it again, and
  younger held messages overtook it. ADR 0256 said Continue would release it,
  which was false once the release had landed.
- **An event-wait resolution.** `EventWaitDelivered` or `EventWaitExpired`,
  then its `PromptInjected` re-entry anchor. If the anchor failed, the dispatcher
  could not re-arm the wait (it was resolved), and the boot sweep only re-drives
  a resolution whose anchor exists. Only a manual Continue recovered it. ADR
  0266 accepted the window.

A race audit filed both under "a marker written before the thing it records".

## Decision

The bus gains an *atomic emit*, `EventBus::emit_atomically(thread_id, events)`.
It writes several persisted events on one thread in one transaction, all or
none, and broadcasts each in order after the commit. Both pairs above are now
one atomic emit. Every persisted thread emit runs the same pipeline, with one
event.

## Rationale

**The window is gone, not covered.** Both rows commit together or neither does.
No retry, sweep or re-queue is needed, and the dispatcher's "anchor missing"
branch is deleted rather than moved.

**Each event keeps its own pipeline.** Validate, Persist, Project and
CaptureAggregate run once per event inside the transaction. So each broadcast
carries its own post-state snapshot. A Validate guard on either event still
works, such as the held-release unique index or the `ChangeApplied` dedupe. A
suppression or an error on any event drops them all.

**One thread only.** The thread append lock is taken once, so the events get
consecutive sequences on that thread. Nothing here needs two threads.

## Consequences

- A held message is either still held or received. A refused delivery leaves
  it at the head of the held list, and Continue releases it, as ADR 0256 says.
- An event-wait resolution never lands without its anchor. A failed pair
  leaves the wait live, and the dispatcher re-arms it. ADR 0266's "narrower
  window" between a resolution and its anchor is closed too.
- No wire change and no frontend change. Old rows have the same shape as new
  ones.
- The held-delivery sleep-polls (`AGENT_START_GRACE`, `AGENT_POLL`) are not
  this window. They wait for a spawned session to register. The spawn
  placeholder from the "claim at entry" work will remove that wait.

## Alternatives considered

- **The resolution is the wake.** `EventWaitDelivered` becomes the turn
  starter, with no `PromptInjected`. Rejected: the lifecycle class is per
  event type, and every old resolution is followed by its own anchor. Old
  threads would grow an extra exchange, unless the starter depended on a
  payload field. It would also move many readers of the anchor: its
  projection effects, the lost-wake sweep, the held-deliveries query, standing
  apply, the todo consumer, the eval starter list and three frontend modules.
- **`MessageReceived` carries the released held-message id.** One event
  releases and delivers, and the held query tests for it. Workable, and it
  would make the frontend's release pairing exact. Rejected once the atomic
  emit existed for the event wait: it needs a wire field, a second unique
  index and a second path for old rows, for the same guarantee.
- **Detect the orphan and re-queue it.** Rejected: that is the patch the audit
  asked us to remove.
