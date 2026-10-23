# 0389: UserPromptInjected is renamed PromptInjected; a migration rewrites rows and subscriptions, and the serde alias stays only to refuse the retired name

- **Status**: Accepted
- **Date**: 2026-10-08

## Context

The `UserPromptInjected` thread event carries `mode: ActorMode`, which is
human, agent or engine. A parent agent injects a follow-up into its child
thread with it. The engine injects a resume note, and every event-wait
re-entry anchor (ADR 0047). So "User" in the name was false, and a name must
describe what the thing is now (`.claude/rules/glossary.md`).

The name had also hidden a bug. The `thread_summaries` projection counted every
injection as a user action and bumped `last_user_action`, the drawer's sort
key. So an event-wait delivery moved a thread in the Saved section.

The event is persisted. Rows carry the name, and a trigger's `on:` list or an
event wait's subscription can name it. Matching is exact string equality.

## Decision

1. The event is now **`PromptInjected`**. The sender lives in `mode`, so the
   name carries none.
2. **A migration rewrites the rows** (`event_type`, and `payload.type` where an
   old row still carries one). It also
   rewrites every subscription naming the old event: `on[]` in
   `TriggerCreated`, `TriggerUpdated`, `EventWaitStarted` and `EventWaitCanceled`
   payloads, the `EventWaitDelivered.event_type`, and `thread_summaries.live_event_waits`.
3. **The serde alias stays**, and `UserPromptInjected` joins
   `ThreadEvent::LEGACY_TYPE_NAME_ALIASES`.
4. A `PromptInjected` bumps `last_user_action` only when `mode` is `Human`.
   Otherwise it bumps `last_agent_action`.

## Rationale

The migration alone keeps every row readable. Migrations run at boot before the
engine serves, and a restored backup migrates on its next boot. So the alias
is not for reading.

The alias keeps the old name **retired rather than free**. A retired name is
refused at subscription registration with a pointer to the new one, and denied
at `emit_event`. A free name is accepted as a possible domain event. So a stale
agent or knowhow could register a wait on `UserPromptInjected` that nothing
ever matches, and learn of it only at the timeout. The drift test in
`thread_events_tests/event_type.rs` only allows serde aliases on the retired list,
so the alias is the price of the refusal: one attribute.

Rewriting subscriptions in the migration keeps the matcher exact. The
migration can reach every `on:` list a trigger or a wait holds. Triggers are
event-sourced, and `trigger.toml` is only a projection of them.

## Consequences

- A trigger or a live wait on the old name keeps firing across the upgrade.
- A new subscription on `UserPromptInjected` is refused, naming
  `PromptInjected`.
- A plugin that ships a `trigger.toml` naming the old event fails to register
  it. That failure is loud, and the fix is the plugin's.
- Prose inside payloads that quotes the old name stays as written, and so does
  the matched event named in a past trigger run's invocation.
- `PromptInjected` is now reserved. A workspace that had emitted a domain
  event under that name would see its subscriptions also match the thread
  event.
- The dated `docs/plans/**` keep the old name as history.

## Alternatives considered

- **Alias only, no migration.** Every SQL reader that names the type as a
  literal (`summary_tree::log::ENTRY_EVENT_TYPES` and others) would have to
  list both names, permanently.
- **Migration only, no alias.** Readable, but the old name becomes free, so a
  stale subscription is silently accepted and never matches.
- **Canonicalize retired names at parse time** (`parse_event_subscriptions`
  maps old to new). It covers subscriptions outside the database, such as a
  plugin's shipped `trigger.toml`. But it needs a new retired-to-live map,
  and it lets the old name register silently instead of being refused.
- **`TurnPromptInjected` or `ThreadPromptInjected`.** The event can also start
  a turn, and every thread event is already scoped to a thread, so neither
  qualifier adds information.
