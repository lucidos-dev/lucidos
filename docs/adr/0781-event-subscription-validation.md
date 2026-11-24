# 0781: Triggers and thread event waits share one subscription shape and one synchronous validation check

- **Status**: Accepted
- **Date**: 2026-11-22
- **Related**: [0047](0047-event-wait-is-an-event.md), [0062](0062-condition-scopes-to-a-thread-without-storing-it.md), [0113](0113-persisted-system-events-are-subscribable.md), [0119](0119-condition-language-field-paths.md), [0131](0131-a-condition-path-is-corroborated-at-subscription-time.md)

## Context

A trigger's `on:` list was the only event subscriber, so its shape lived in
`triggers/config.rs`. A thread's event wait became a second subscriber. The
shape moved to `core/event_subscription/mod.rs` because the old name no longer
described what it was.

A misspelled event name arms cleanly and never matches. An unsupported operator
does the same. A trigger on `TextStreamed` validates, persists and then never
fires.

This record restates the design comments of
`crates/lucidos-core/src/core/event_subscription/mod.rs`, written before the
comment strip removes them. Nobody checked it line by line against the code.

## Decision

- Triggers and thread event waits share one `{event_type, condition}` shape,
  `EventSubscription`, and one matcher, `EventSubscription::matches`.
- The shared contract also includes the subscribability gate, with one function
  per carrier. `is_subscribable` drops the per-token streaming firehose before
  a matcher sees a thread event. `is_subscribable_system_event` decides which
  system frames reach the matchers at all.
- `matchable_payload` decides what a `condition` is evaluated against. Every
  consumer that offers a payload builds its view there.
- The `EventWait*` family is triggerable but never awaitable.
  `SubscriptionSurface` has two values, `Trigger` and `Wait`, because this is
  the one family the surfaces disagree about.
- A name that is not a `ThreadEvent` name is accepted, because it may be a
  domain event nobody has emitted yet.
- A refused subscription is an error at the tool boundary, never a silent
  no-op. The error names the blocked variant so the caller can pick another
  event in the same turn.
- One check runs at every subscription site: `await_event`, the trigger HTTP
  API and the trigger LLM tool. It validates the `event_type` and the
  `condition` of every entry.
- The first failure refuses the whole call.
- The misspelling check is split from `validate_subscribable_event_type`. The
  caller consults the event store first and runs it only for a name nobody has
  emitted.
- The `{"type": ..., "data": {...}} envelope of a persisted `SystemEvent` is
  flattened before matching. The gate is the event name first, then the shape.

## Rationale

- One matcher and one gate mean a `condition` that fires for a trigger also
  fires for a wait. The two consumers cannot drift.
- A wait on `EventWaitStarted` self-satisfies on the next registration anywhere
  in the workspace. A wait on `EventWaitDelivered` self-satisfies the instant
  any wait delivers. A trigger that notifies about a timed-out wait is still
  reasonable, so the family stays triggerable.
- The accepted sets are nested. `Trigger` accepts everything `Wait` does, plus
  `EVENT_WAIT_EVENT_TYPES`. A suggestion drawn from the wait corpus is
  therefore usable at either surface, and the near-match heuristic stays
  surface-free.
- Both write surfaces are synchronous, so only a synchronous refusal turns a
  silent failure into something the caller can fix at once.
- A partly armed subscription reads as a success while watching for less than
  the caller asked. Refusing the whole call avoids that.
- A workspace that emitted `CredentialStored` as its own domain event owns that
  name. A misspelling heuristic must not take it away, and ownership is a
  question about the event store, not the name.
- Refusing unemitted domain names would make the tool useless for the
  workspace-defined events it is most useful for.
- Every stored condition on a system event names the field directly, so
  flattening the envelope stays. A field path reaching `data.filename` does not
  make it optional.
- The shape alone is ambiguous. A workspace domain event authored as `{"type":
  "ReleasePublished", "data": {...}}` would lose its own payload to `data`. A
  workspace cannot emit a reserved name, because `is_reserved_type_name`
  refuses the whole reserved set at the emit endpoint.

## Consequences

- A subscription refused by one site is refused by all three.
- Refusal messages are surface-neutral, because every case but one is shared.
  Wording such as "a wait would never resolve" would describe machinery a
  trigger does not use.
- A syntactically valid field path that no payload of that type carries cannot
  be decided from the subscription alone. `uncorroborated_path_warnings` asks
  the event store and warns rather than refuses.
- A name the workspace has never emitted gets a note in the tool result.
- Live matching and replay pass the same stored name, so both flatten
  identically. Thread events and workspace domain events pass through
  untouched.
- The agent reads the same flattened value back, because
  `delivery_reentry_text` prints it.

## Alternatives considered

- Accept a doomed subscription and let it never fire. Rejected as a documented
  footgun the user finds days later.
- Arm part of a subscription and report success. Rejected, because it watches
  for less than the caller asked.
- Drop the envelope flattening because a field path could reach
  `data.filename`. Rejected, because it breaks every stored condition on a
  system event.
- Flatten by shape alone. Rejected, because a domain event shaped like the
  envelope would lose its payload.
- Refuse names that are not `ThreadEvent` names. Rejected, because it blocks
  workspace-defined domain events.
- Let the misspelling heuristic decide from the name alone. Rejected, because
  it could take away a name a workspace owns.
