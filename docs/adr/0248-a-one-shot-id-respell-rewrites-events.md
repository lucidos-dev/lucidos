# 0248: A one-shot id re-spell may rewrite the model field on past events

- **Status**: Accepted
- **Date**: 2026-09-22

## Context

Events are immutable and append-only. That is a core architectural principle:
Git is the artifact store but never the authority, and the events table is.

Four chat-registry rows carried a Vertex-only alias: `claude-opus-5@default`,
`claude-opus-4-8@default` and their `[1m]` variants. The direct Anthropic API
rejects that spelling, so a row served by both backends could not keep it as its
identity. ADR 0247 re-spells them to bare ids.

The problem is what reads those ids back. `last_thread_chat_settings` resolves
*per-thread model memory* from the `model` stamped on the newest
`MessageReceived` or `TriggerStarted`. That is not history: it is a live setting
the next turn obeys. Leave it and a thread's remembered model names a row that
no longer exists, so it falls to the id-shape guess and lands on Vertex.

## Decision

A one-shot migration may rewrite the `model` field on past events. It may do so
only for event types whose `model` is **read back as a live setting**, or
rendered as a model's NAME.

Seven types qualify: `MessageReceived`, `TriggerStarted`, `ResponseGenerated`,
`ResponseCanceled`, `ResponseAborted`, `TriggerCreated`, `TriggerUpdated`.

## Rationale

The precedent is `20260416080000_migrate_model_aliases_to_full_ids.sql`, which
rewrote `model` on four event types for exactly this reason. This ADR records
the rule that migration followed without writing down.

**The distinction is what the field is FOR**, not how old the row is. A field a
later read treats as configuration has to stay resolvable, or the configuration
silently changes meaning. A field that records what was spent or sent is a fact
about the past, and rewriting it is falsification.

So three groups of rows are deliberately NOT rewritten:

- `ContextCaptured` and `ContextAssembled`, the cost ledger the Token Cost app
  reads. The request really did name `@default`.
- `CodingAgentSettingsChanged`. Claude Code has its own id vocabulary, from
  `runtime/cc_menu_options.json`, where `claude-opus-5@default` is still the
  correct spelling. That picker is untouched by ADR 0247.
- `JevCallCompleted`, `ImageDescribed` and `ConversationSummarized`. Other model
  namespaces, none of them a chat-registry id.

The alternative to rewriting is a permanent read-time alias, which was weighed
and is recorded below.

## Consequences

- The immutability principle now has one named exception with a test for its
  boundary, rather than an undocumented precedent nobody could cite.
- Adding an event type to a future re-spell requires answering one question:
  is this field read back as a setting, or is it a record of what happened?
- The frontend keeps labels for the `@default` spellings, because they still
  reach the transcript from the two namespaces left alone.
- A migration that rewrites events is bounded to a mapping table it declares, so
  the set of rewritten values is readable in the file.

## Alternatives considered

**Leave events alone and resolve the legacy id at read time.** History stays
byte-for-byte immutable and no exception is needed. Rejected: it is a permanent
back-compat layer in both the engine and the frontend picker, carried forever so
that a one-time rename could be avoided once.

**Leave events alone and accept the drift.** The smallest migration. Rejected
because a thread carrying the old spelling shows a phantom picker entry and
silently loses its remembered model, with nothing saying so.

**Rewrite every event type carrying a `model` field.** Simplest rule to state.
Rejected: it falsifies the cost ledger, and it corrupts the Claude Code picker's
own vocabulary, where the old spelling is still right.
