# 0534: An intent runs in the thread that asks for it, through load_knowhow; execute_intent and its sub-loop are gone

- **Status**: Accepted
- **Date**: 2026-10-20
- **Supersedes**: the `load_app_knowhow` paragraph of [0111](0111-app-know-how-is-a-pointer-not-a-body.md)
- **Amends**: [0088](0088-tool-schemas-are-gated-by-capability-not-trimmed.md) and [0242](0242-every-model-call-records-its-cost.md), where they cite `execute_intent`

## Context

A trigger pinned to Sonnet 5.5 at `medium` called `execute_intent` on its own
procedure. The intent sub-loop ran 33 rounds on Opus 5 at `high`, and cost
most of the run.

The sub-loop called the model with `ModelSelection::default()`. The routing
provider then fell back to its boot default model. So the sub-loop ignored the
turn's model, effort and provider, and even the stored `chat_model`.

The sub-loop was also a cut-down copy of the main agentic loop. It had no model
resolution, no context mode, no tool-result cap and no streaming. Every loop
fix had to be copied into it by hand.

## Decision

`execute_intent` and its sub-loop are deleted. An intent runs in the thread that
asks for it: `load_knowhow(<intent id>)` returns the intent body with its
knowhow, and the turn does the steps itself, on its own route.

A `run_thread` child inherits its caller's model, effort and provider, unless
the call pins its own.

## Rationale

**One loop, one route.** The main loop already resolves the model, honours the
context mode, caps tool results and streams. Running the intent there gives it
all of that for free, and no second loop can drift.

**`load_knowhow` already carries the payload.** An intent is a procedure plus
the docs it names. That is what `load_knowhow` returns, so a fallback from a
knowhow miss to the intent registry is the whole mechanism.

**A doc wins an id clash.** `load_knowhow` resolves the id as knowhow first, as
it always did, so no id that loads a doc today changes meaning. The workspace
audit flags an intent a same-id doc shadows.

**Isolation, where it is needed, is `run_thread`.** That is a real thread with
the real loop. Inheriting the caller's route makes it cost what the caller
already chose.

## Consequences

- The tools array no longer carries the `execute_intent` schema, and
  `Gate::Intent` is gone. The `## Available Intents` prompt section stays.
- `ContextPurpose::IntentLoop` stays as a retired variant with no producer, so
  old `ContextCaptured` rows still read.
- A call to `execute_intent` from replayed history or stale workspace prose
  gets a refusal naming the exact `load_knowhow` call. It is a permanent
  guard, not a temporary measure: its only removal condition would be a scan
  of user history.
- Old `execute_intent` tool pairs still replay and still render.
- Workspaces migrate through the `retired-tools` audit check and the
  `intents-run-in-the-thread` release notice, per ADR 0214.
- The intent's work now shares the asking turn's context window. A long
  procedure that needs its own window goes through `run_thread`.
- Two sibling auxiliary calls still use `ModelSelection::default()`: the
  `correct_memory` verdict and the `import_file` artifact summary. They are out
  of scope here.

## Alternatives considered

**Keep the sub-loop and pass it the turn's route.** It fixes the model, but not
the drift. The sub-loop would still lack the context mode, the tool-result cap
and streaming, and would keep needing every loop fix copied in by hand.

**Run every intent through `run_thread`.** It isolates by default, but every
intent then pays a thread spawn and a fresh context. Most intents are short
procedures the asking turn can just do.

**An intent wins an id clash.** It would silently change what an existing
`load_knowhow` id returns. A doc winning keeps every id stable, and the audit
makes the shadowed intent visible.
