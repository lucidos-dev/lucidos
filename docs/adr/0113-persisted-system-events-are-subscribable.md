# 0113: Persisted system events are awaitable and triggerable

- **Status**: Accepted
- **Date**: 2026-08-24

## Context

`await_event(on=[{event_type: "BackupCompleted"}])` used to be refused at the
tool boundary. The refusal said the wait matcher sees thread events and domain
events, so a wait on a system event would never resolve. It then advised waiting
on "the thread event or the domain event that accompanies it".

That advice pointed at nothing. `BackupCompleted` and `BackupFailed` have no
companion of either kind. They are durable facts: `SystemEvent::is_persisted()`
is true for both, `EventBus::persist` writes them to the `events` table, and
`core::backup::load_recent_runs` reads the rows back as the run history behind
`get_backup_status`.

So "tell me here when tonight's backup finishes" was impossible, and so was a
trigger on a failed backup. The same held for every other persisted
`SystemEvent`: 69 names in `PERSISTED_TYPE_NAMES`.

The trigger side had the same hole from the other direction. The scheduler
forwarded only `SystemEvent::DomainEvent` to the trigger matcher. So a workspace
could react to a name it invented, but not to one the engine writes.

## Decision

**A `SystemEvent` that is persisted is awaitable and triggerable. A transient
one is neither.**

`is_persisted()` draws that line already and is the maintained source of truth,
so the gate keys on it rather than on a new hand-kept list. One predicate,
`core::event_subscription::is_subscribable_system_event`, answers the question
for both fan-outs: the event-wait dispatcher and the scheduler's trigger
forward. It admits any `DomainEvent` plus anything `is_persisted()` accepts.

Three consequences of that shape are load-bearing:

- **The matchable payload is the stored row, flattened.** `SystemEvent` is
  adjacently tagged, so a row's `payload` is `{"type": …, "data": {…}}`. The
  condition language reads one flat level and has no dot paths. Both the live
  path and the replay path therefore unwrap the envelope, which is what makes
  `condition: {"filename": …}` mean the event's own field. The unwrap is gated
  on the name being a persisted `SystemEvent` name, because only those write the
  envelope. A workspace can author a domain payload in that exact shape, and
  unwrapping it would replace what the workspace wrote with its `data` value.
- **A frame fans out at the depth of the task that emitted it.** A trigger's own
  run emits system frames, and a trigger may subscribe to one it emits. That is
  a cycle, and the existing `MAX_EVENT_TRIGGER_DEPTH` cap is what ends it. So
  `EmittedEvent` carries `depth`, read at emit from the same task-local a
  `DomainEvent` already stamps itself from. Restarting the chain at 0 for engine
  frames would let such a trigger call itself forever.
- **The emit guard is untouched.** Waiting on a name and being allowed to emit
  it are different permissions.

## Rationale

Persistence is already the exact test. A persisted frame writes a row, and a row
is what both matchers can see, live and on replay. A transient frame writes
nothing, so a wait on it could only ever expire. Keying the gate on
`is_persisted()` gives each new variant one decision instead of two. Making it
durable is what makes it subscribable, so there is no second list to forget.

One predicate rather than two expressions is what makes invariant I8 structural.
The wait matcher and the trigger matcher must return identical verdicts for the
same event. Two copies of `matches!(se, DomainEvent{..}) || se.is_persisted()`
would agree today and drift later.

The refusal for a transient frame now says the frame is transient, and names the
persisted terminal event where one exists. `BackupProgress` points at
`BackupCompleted or BackupFailed`. That is advice, so a frame with no terminal
twin gets the shorter message.

## Consequences

- `await_event` accepts every name in `SystemEvent::PERSISTED_TYPE_NAMES`, and a
  trigger's `on:` list accepts the same set.
- Transient frames stay refused at registration, so they never reach either
  matcher. `BackupProgress`, `Toast`, `MemoryRebuildProgress`,
  `RecoveryProgress` and `EmbeddingModelStatusChanged` are in that half.
- A system frame belongs to no thread. Its row's `thread_id` column is NULL, and
  no thread id is injected at match time, so a `thread_id` condition on one
  matches nothing. This mirrors the existing rule for domain events.
- `RESERVED_TYPE_NAMES` and `is_reserved_type_name` keep their one job: refusing
  a forged name at `POST /api/v1/events/emit`. A test pins that every persisted
  name is now awaitable AND still un-emittable.
- A trigger subscribing to a frame its own run emits, `TriggerCompleted` being
  the obvious one, stops after `MAX_EVENT_TRIGGER_DEPTH` hops. **Superseded by
  ADR 0137**: it is now never woken by what its own fire emits. What the cap
  still ends is a chain across triggers, or one running through work a fire
  handed off. The rest of this ADR stands.
- Both carriers read the same depth. `scheduler::trigger_dispatch` answers a
  thread event and a system frame in one place, off `EmittedEvent::depth`.
  `thread_queue::executor` scopes the whole fire, so the `TriggerExecuted` the
  fire records carries the fire's depth too. Recording it after the scope
  resolved stamped 0, and left a trigger subscribed to it uncapped.
- A name that is both a `ThreadEvent` and a `SystemEvent`, such as
  `ChangeDiscarded`, still resolves to the thread event. That branch stays first
  in `validate_awaitable_event_type`, because only the thread-scoped one can be
  scoped by a `thread_id` condition.

## Alternatives considered

**Keep the envelope verbatim in the matchable payload.** Rejected. The condition
language reads one flat level. A condition would have to name `type` and `data`,
and it could not reach a field inside `data` at all. Adding dot paths is a much
larger change to a language both matchers share. It would also make every
existing condition read against a different shape per carrier.

**Add a dedicated allowlist of subscribable system events.** Rejected. It
restates `is_persisted()` and rots the first time somebody adds a variant. The
whole point of keying on the existing predicate is that there is one decision to
make per variant, not two.

**Widen the emit guard so the two questions share one list.** Rejected outright,
and it is the reason the predicates are split. `RESERVED_TYPE_NAMES` exists so
an untrusted app cannot POST a fake `NotificationCreated`. Relaxing it to make
waiting work would trade a real security boundary for a convenience.

**Leave the refusal and tell people to emit a domain event beside the backup.**
Rejected. It asks every workspace to duplicate a fact the engine already
recorded, and it cannot work for engine-internal runs nobody scripted.
