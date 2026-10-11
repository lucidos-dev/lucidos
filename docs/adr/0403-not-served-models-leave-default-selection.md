# 0403: A provider's not-found answer drops a model out of default selection for six hours, and the user sees it

- **Status**: Accepted
- **Date**: 2026-10-09

## Context

Google lists Claude Haiku 4.5 on Vertex as retiring not sooner than Oct 15,
2026. Haiku 4.5 was the command judge's catalog default, first on the judge's
recommended list.

Default selection picked the first recommended model a *configured* provider
had a route for. Nothing checked that the model still answered. So a
Vertex-only workspace would keep choosing Haiku 4.5 after the retirement, and
every judge call would 404.

The 404 was only text. `explain_publisher_model_404` rewrote it into advice,
but no caller could tell it from any other failure. The judge's caller fell
back to the static classifier and logged one line. Settings kept showing the
model as reachable.

The router does not help either: ADR 0247 refuses to move a call to another
route of the same model.

## Decision

1. **A provider's model-not-found answer is a typed error,
   `llm::ModelNotServed`.** Vertex's publisher-model 404, Anthropic's
   `not_found_error`, and the OpenAI-compatible not-found shapes return it.
   An auth, quota or path failure does not.
2. **The auxiliary call that meets it records `ModelNotServedObserved`.**
   At most once per provider, model and purpose within six hours.
3. **For six hours an unset default passes over that model on that
   provider.** Selection reads the window from the events on every call.
4. **The call that met it retries once on the next reachable recommended
   model.** Later requests through the same provider go straight there, and
   the cost record names the model that ran.
5. **A stored pick is never moved.** It fails, its Settings row says the
   provider no longer serves it, and one notification per window names it.

## Rationale

**Default selection already chooses between models and vendors**, so moving
past a model there honours ADR 0247. A stored pick and a turn's route are the
user's choice, and they stay honoured or refused.

**The events table is the record**, so the window survives a restart with no
projection to rebuild. `idx_events_type_created` makes the per-call read an
index range scan.

**Six hours, then one fresh try.** Enabling the model in the provider's
console takes effect with no restart. A retired model costs one fast 404
every six hours per purpose.

**The retry keeps the first call whole.** Without it, the call that met the
404 failed. For the command judge that would mean a permission card each
window, for no reason the user caused.

## Consequences

- A retirement no longer strands an unset background task, the command judge
  included. A judge default can move to a model the provider has not enabled
  yet, and the first 404 moves it on.
- Settings names what it moved past, and marks a stored pick its provider
  refuses.
- Turns are unchanged. A chat model that 404s fails loudly already.
- The compactor gets no in-call retry. It re-resolves per node, so its next
  node starts on the next model, at that model's own measured tier.
- `ModelNotServed` is the second blessed custom error type in
  `.claude/rules/rust.md`.

## Alternatives considered

**Fall through to the model's next route.** Rejected by ADR 0247: Vertex and
Anthropic are different vendors with different data terms, and moving a call
between them silently undoes the user's choice.

**Probe every recommended model at startup.** It spends money and boot time
on every start, and still misses a retirement that lands while the engine
runs. Real traffic finds the same thing for free.

**An in-memory record.** Simpler at first, but a restart forgets it, and
every restart then re-learns each retirement with a failed call.

**Swap the defaults by hand at each retirement.** That is what the Haiku 5.5
plan does for the judge, gated on a measured check. It cannot help a
workspace whose provider has not enabled the replacement, which is the case
this decision covers.
