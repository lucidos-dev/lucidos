# 0131: A condition's field path is corroborated against stored payloads when you subscribe, and warned about rather than refused

- **Status**: Accepted
- **Date**: 2026-08-25

## Context

ADR 0113 and the subscription-time name check ended one silent failure: a
subscription on an event type the engine never emits. A second one survived it,
one level down.

A `condition` was validated for syntax only. A field path that is perfectly
formed and names something no payload of that type carries arms clean, matches
nothing, and reports nothing. ADR 0119 fixed the resolver's semantics here on
purpose: a path that resolves to nothing is JSON null, exactly like a missing
top-level key. That is right for matching and useless for diagnosis.

The incident. The Lucidos Agent armed an `await_event` on `PluginInstalled` with
`{"id": "pull-requests"}`. That payload had no top-level `id`. The plugin id sat
at `manifest.manifest.id`, two layers down. The variant's `manifest` field
carries an install record, whose own `manifest` key holds the raw manifest. The
wait sat armed while the install landed, and the agent found its own mistake
hours later.

Guidance had already been written and had already failed. `triggers.md` tells
the reader to query one real stored event and write every path from what it
shows. That text was live when the agent guessed.

## Decision

`check_subscriptions` corroborates every field path in a `condition` against the
twenty most recent stored payloads of that event type, each run through
`matchable_payload`. A path that resolves in none of them produces a **warning**
naming the real path, never a refusal.

`SystemEvent::PluginInstalled` also gains a top-level `id`, matching every
sibling `Plugin*` frame.

## Rationale

**Empirical, because there is no schema to check against.** `PluginInstalled`
carries its manifest as a bare `serde_json::Value`, and a domain event's payload
is whatever the workspace wrote. Nothing in the type system knows the shape. The
event store does, so it is the only thing that can answer.

**A warning, because a sample is evidence and not a schema.** An optional field
is legitimately absent from twenty rows: `actor` is `skip_serializing_if`, and
`{"conclusion": {"$ne": "success"}}` deliberately matches an event with no
`conclusion` at all. A refusal would block both. Naming the real path is what
makes a warning enough, and the sample is already in hand.

**One chokepoint, so it reaches everything.** `check_subscriptions` is run by
`await_event` registration, by trigger create and update over HTTP, and by the
trigger LLM tool. All of them already render its warnings, so the check needed
no new plumbing and no surface can miss it.

**Through `matchable_payload`, so the probe sees what the matcher will see.** A
probe reading the raw row would flag `thread_id`, which is injected rather than
stored. It would also miss the `type` / `data` envelope the matcher strips. Both
would be false alarms on correct conditions.

**The top-level `id` removes the trap rather than reporting it.** Its siblings
all carry one, and the frontend already special-cased its absence. Worse,
`aggregate_id()` derived it two levels down with an `"unknown"` fallback that has
produced broken rows before. The obvious filter is now simply right.

## Consequences

- One indexed query per subscription entry that carries a condition, inside a
  synchronous tool call. Refused names never reach it.
- A false alarm is possible on a genuinely optional field. The message says the
  path is in none of the recent payloads, not that it is wrong.
- An event type with no stored rows says nothing. The never-emitted warning
  already covers that case with better information.
- An unreadable store says nothing either. A probe that could not run is
  unknown, never a no.
- An additive payload field warns until the next event of that type lands, since
  the sample is all older rows. It self-heals on the first new one, and
  `PluginInstalled.id` is the first instance.
- At most three paths are named per entry, with any remainder counted in one
  more line rather than dropped.
- `resolve` returns `Option` now, so present-versus-absent is answerable. The
  evaluator unwraps to the same JSON null it always read, so no stored condition
  changes verdict.

## Alternatives considered

**Refuse an uncorroborated path.** Rejected. It would break
`{"conclusion": {"$ne": "success"}}` and every other deliberate filter on an
optional field, and a sample can never prove absence.

**Derive a static payload schema from the Rust enum.** Rejected. It cannot see
inside a `serde_json::Value`, which is exactly where the incident's field lived,
and it knows nothing about a workspace's own domain events. It would answer
confidently for the easy cases and stay silent for the hard ones.

**A tool action returning an event type's shape.** Rejected as redundant. The
`events` tool's `query` action with `limit` 1 already returns a real payload, and
the warning names the path without being asked. A second surface would need the
agent to think of consulting it, which is the step that failed here.

**A sentence in the `await_event` tool schema.** Rejected on cost. It broke
`ALWAYS_LOADED_BUDGET_CHARS`, and `await_event` is already the largest schema in
the prompt. Guidance there is billed on every request of every thread, where the
warning is billed only when someone gets it wrong.

**Rename `PluginInstalled.manifest`, which carries an install record.** Rejected.
The name is wrong, but the rows are append-only and every reader resolves that
path. A rename buys clarity in the enum and breaks the stored history.

**More documentation.** Rejected as already tried. `triggers.md` carried the
right rule before the incident and the agent still guessed. The knowhow was
sharpened in the same change, but as the explanation beside a deterministic
check rather than instead of one.
