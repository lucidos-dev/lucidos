# 0062: A condition can scope to a thread without the thread id being in the payload

- **Status**: Accepted
- **Date**: 2026-08-12

## Context

An *event subscription*'s `condition` is evaluated against an event's payload,
and no `ThreadEvent` payload carries the thread the event belongs to. The id
lives on the carrier (`BusEvent::Thread { thread_id, event }`) and in the
`events.thread_id` column, and `20260314120000_strip_thread_id_from_payloads.sql`
deliberately removed it from stored payloads because the column is the source of
truth.

So "wait until that coding-agent session finishes" had no mechanism. The chat
system prompt claimed it did, telling threads to scope `CodingAgentIdled` with a
`child_thread_id` condition, which is a field on `ChildThreadCompleted` and only
the parent/child fan-in emits that. A condition naming an absent field never
matches, so the wait never fired and the thread never woke. The user asked for
this five times in one day and it failed five times.

The gap was wider than one variant. Every thread event had it, and the two
matching paths did not even agree with each other: the live dispatcher matched
`to_payload(&EventMeta::NONE)` while the catch-up scan read the persisted
payload, which additionally carries whatever meta the emit stamped.

## Decision

The payload a `condition` is evaluated against is a **matchable payload**: the
event's own serialized fields plus `thread_id`, injected at matching time from
the bus carrier on the live paths and from the `events.thread_id` column on the
replay paths. `core::event_subscription::matchable_payload` builds it and every
consumer that offers a payload to `EventSubscription::matches` goes through it.

Nothing new is persisted. The stored payload is unchanged, and the strip
migration stands.

## Rationale

The thread id is not a missing field, it is a missing view. It already has two
canonical homes, and both matching paths can read one of them authoritatively,
which is exactly the property a conditionable field needs: the live path has the
carrier, the replay path has the column, and they cannot disagree.

Injecting it makes every thread event scopable at once, which is the point. A
per-variant field would have fixed the one event in front of us and left the
next one unscopable, at the cost of duplicating a column into every row forever.

Doing it in one function is what keeps the four paths honest. The live wait
dispatcher, the catch-up scan, the arming lookback and trigger dispatch each
build their own view, and a condition that means one thing to a trigger and
another to a wait is precisely what `core::event_subscription` exists to
prevent.

## Consequences

- `condition: { thread_id: "<uuid>" }` works on any thread event, for a *trigger*
  and for an *event wait* alike. The prompt, the `await_event` schema and the
  shipped knowhow all say so.
- A field a condition can name does not appear in the payload read back from
  `query_events`. The knowhow states this rather than leaving it to be
  discovered.
- Injection is insert-if-absent, so an event that owns the key keeps its value.
  No variant declares one today; `ChildThreadCompleted.child_thread_id` names a
  different thread and is unaffected.
- The `events` table does not grow, and no index is needed: the id is never
  queried out of a payload.
- **A domain event is deliberately not thread-scopable.**
  `SystemEvent::DomainEvent` carries no thread and its row's column is NULL, so
  a condition on `thread_id` matches nothing there. Supplying an originating
  thread on the live path alone would recreate the live-versus-replay split this
  decision closes. If a workspace's own `emit_event` should record its calling
  thread, that is a change to the event, not to the matcher.
- **`EventMeta` fields stay unconditionable**, one step further out for the same
  reason: `EmittedEvent` does not carry the meta to live subscribers, so `actor`
  and `channel` could only ever be matched on replay.
- **One exception to the live/replay parity, and it is deliberate.**
  `EventBus::replay_historical_event` writes a row that carries a `thread_id`
  and broadcasts it as a system `DomainEvent`, because a backfilled historical
  event is not something that just happened on a thread and must not run the
  trigger matcher or the fan-in as though it had. So a `thread_id` condition
  cannot match that live frame, while the scan reading the row it just wrote
  will inject one. The direction is the safe one: the wait resolves from the
  scan rather than never, and the only caller is a startup backfill, where the
  scan is what covers the path anyway. Making the frame a `BusEvent::Thread`
  instead would fix the parity by giving every backfilled row the full live
  fan-out, which is a far larger behaviour change than the asymmetry costs.

## Alternatives considered

**A `thread_id` field on `CodingAgentIdled`.** The original brief. It fixes the
reported failure and nothing else: the next event type needs the same edit, and
each one costs every construction site in the tree (59 for this variant alone).
It also re-adds to a payload the exact key the 2026-03-14 migration removed from
payloads, duplicating a column into every row of that type forever.

**A composed `CommonThreadAttributes` struct flattened into many variants.** The
scaling version of the same idea, and it inherits the same storage cost across
more events while touching every construction site of every variant it lands on.
It is also a solution to a problem that is already solved: the composed
cross-cutting struct is `EventMeta`, and the thread id is not in it precisely
because it lives on the carrier and in a column.

**Carrying the thread id in `EventMeta`.** Rejected against the dispatcher's own
comment: the live matcher sees `EventMeta::NONE` while the catch-up scan reads
the persisted payload with the meta merged in, so a meta-carried id would match
one path and not the other. The asymmetry, not the mechanism, is what rules it
out.

**Passing the thread id into `EventSubscription::matches` as a parameter.**
Forces every call site to supply it, but re-injects per subscription rather than
once per event, and spreads the "what a condition can name" decision across the
callers instead of holding it in one place.
