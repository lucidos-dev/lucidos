# 0240: A title event names an existing thread row and never creates one

- **Status**: Accepted
- **Date**: 2026-09-21

## Context

`ThreadTitleGenerated` and `ThreadTitleRenamed` are projected into
`thread_summaries` by a bare `UPDATE ... WHERE thread_id = $1`. An event for a
thread whose row does not exist yet matches nothing, and the name is lost.

That is not hypothetical. Both thread-spawn paths used to emit the caller's
chosen title before the thread's first `MessageReceived`. The name was dropped,
and the auxiliary title model renamed the thread a second later. It renamed 312
of 609 coding-agent threads in one workspace over 30 days.

The obvious repair is to make the title arm an upsert, so the name cannot be
lost whatever the order. It is the wrong repair, and the reason is not local to
the title arm.

## Decision

The title arm stays an `UPDATE`. A title event names a thread that already
exists, and a caller that emits one earlier is the bug.

The arm reports a write that matched no row, so the drop is loud rather than
silent. `write_thread_title` in
`crates/lucidos-engine/src/engine/event_bus_projection_thread.rs` returns that
answer and its caller logs it with the thread id.

## Rationale

`MessageReceived` is what creates a thread's row, and only its INSERT path
writes the thread's identity: `parent_thread_id`, `depth`, `initiator` and
`spawning_event_id`. Its `ON CONFLICT DO UPDATE` arm sets none of them, on
purpose, because an ordinary follow-up message carries no parent and must not
clobber the stored one.

So a row conjured by an earlier title event does real damage. The real
`MessageReceived` then lands on the conflict arm. The sub-thread is left
unlinked from its parent, at depth 0, with the wrong initiator, while the
parent's `active_children_count` still incremented. A wrong thread name is
cosmetic. A sub-thread that has lost its parent breaks the callback chain, the
drawer's tree and the blocking and attention counts.

The frontend settled the same question the same way in `d91c61b36`. An SSE
event with no aggregate was conjuring a drawer row, and the skeleton it built
was a titleless entry that a reload swept away.

## Consequences

- Ordering is a contract that emit sites owe, not one the projection repairs.
  `a_spawn_does_not_name_its_own_thread` holds the two spawn sites to it, and
  `an_early_title_does_not_cost_a_sub_thread_its_parent` pins the damage the
  upsert would do.
- A mis-ordered emit still loses its name. The log line is what makes the next
  one findable on its first spawn instead of after three hundred.
- A thread hard-deleted while a title call is in flight logs the same line. It
  is accurate (there is no row) and harmless.

## Alternatives considered

**Upsert the row from the title event.** Rejected above: it costs a spawned
sub-thread its parent linkage, which is worse than the name it saves.

**Widen `MessageReceived`'s conflict arm** to fill the identity columns with
`COALESCE`, then upsert freely. This would work. It is the version to revisit
if a second caller ever needs to name a thread before it exists. Rejected for
now on blast radius: that arm is the hottest projection path in the engine and
runs on every message of every thread. `depth` is `NOT NULL`, so it needs a
`CASE` rather than a `COALESCE`. No caller needs it.

**Keep the silent drop.** Rejected. It is the property that let one mis-ordered
emit site rename hundreds of threads without a single line in the log.
