# 0312: A thread is never pinned and archived at once; automatic archiving skips pinned threads

- **Status**: Accepted
- **Date**: 2026-09-27

## Context

A thread carries two retention facts: `is_saved` (the user's *pin*) and
`archive_state`. They were independent, so a thread could be both. The drawer
showed such a thread under Pinned, because `display_section` checks the pin
first. `available_thread_actions` offers Archive only in the inbox, so the user
saw Pin and Unpin and no way to archive it.

The state came from pinning an archived thread. `ThreadSaved` moved no section,
so the thread stayed archived underneath the pin.

## Decision

Pinned and archived are exclusive, and the engine enforces it:

1. **Pinning an archived thread moves it to the inbox**, in the same event.
2. **The user's own Archive unpins**, in the same event, after the drawer
   confirms it.
3. **Automatic archiving skips a pinned thread.** It stays in the inbox, still
   pinned. Today that is the unattended-trigger rule in `resolve_transition`.
   Any future sweep follows the same rule.
4. **Discarding a draft unpins it.** That archive is housekeeping, not a user's
   choice.
5. **An agent never archives a pinned thread** (the agent archive of ADR 0310).
   A pinned target is refused with `thread_pinned`, and a pinned sub-thread is
   left open while the rest of the family goes. A self-archive request waits
   while its thread is pinned. The user decided this: a pin is theirs, and an
   agent's judgement that the work is done does not outrank it.
6. **The database refuses the pair**, through a `CHECK` constraint on
   `thread_summaries`.

Existing pinned-and-archived rows were repaired by keeping the pin and moving
the thread to the inbox.

## Rationale

A pin means "keep this at hand". An automatic sweep has no user intent behind
it, so it must not undo one the user expressed. An explicit Archive is the
user's own later act, so it wins over the older pin.

The repair keeps the pin for the same reason: in every such row the pin came
after the archive. Replaying the events under rule 1 yields the repaired row, so
the event log and the projection agree without a new event.

The `CHECK` constraint makes a missed path fail its emit loudly rather than
write the state silently. The contract fixture drops the illegal pair, so the
TypeScript mirror is never tested against a state the engine cannot hold.

## Consequences

- Archive is offered on a pinned, settled thread with no special case, because
  a pinned thread always sits in the inbox.
- A pinned trigger thread keeps surfacing its runs in the inbox, even when
  nobody opted into review. Pinning it is the opt-in.
- A new write path that archives must decide what it does with a pin. The
  constraint forces that decision at the first test run.

## Alternatives considered

- **One retention enum** (`Pinned | Inbox | Archived`) in place of two columns.
  It makes the pair unrepresentable in the type system too. It lost on cost: it
  renames a column, the wire field and every frontend reader, for a guarantee
  the constraint already gives at the storage layer.
- **An automatic sweep that unpins as it archives.** It treats a sweep like the
  user's Archive. It lost because it silently discards the user's intent.
- **Repair by unpinning.** It loses the user's newer act, and it disagrees with
  what replaying the events now produces.
- **UI-only enforcement.** The engine would still store the pair for any other
  caller, such as the SDK or a future sweep.
