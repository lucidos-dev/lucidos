# 0390: An unset reasoning tier runs at the model's documented default; a task's tier binds its recommended models; chat remembers a tier per model

- **Status**: Accepted
- **Date**: 2026-10-08

## Context

With no tier stored, the engine picked one tier for every model. Chat sent
`high`. Background tasks sent their task's tier whatever the model. Claude and
Gemini requests with no effort were filled in with `high` at the wire. The
providers document different defaults: Opus 5.5 defaults to `medium`, Opus 5
to `high`, GPT-5.4 to `none`. So a model switch could quietly double or halve
the thinking a user paid for.

Plan: `docs/plans/2026-10-08-each-model-runs-at-its-default-effort.md`. This
supersedes the unset-effort clause of ADR 0368, which resolved a missing chat
effort to the catalog's `high` on every route.

## Decision

1. Each model row carries a *default effort*, seeded from the provider's
   documented default. A row with none sends no effort at all.
2. Chat stores a tier per model (`chat_reasoning_efforts`). A model with none
   stored runs at its default effort. A thread's remembered tier stays with the
   model it ran on.
3. A background task's own tier binds the models on its recommended list. Any
   other model runs at its default effort.
4. The compactor keeps its measured `low` on its three measured models.

## Rationale

- **Explicit, so it can be shown and priced.** Sending no effort would let the
  provider choose, but then the engine cannot say which tier ran. The picker
  could only read "provider default", and the Tree estimate could not price the
  default. A tier on the row is sent, displayed and priced from one value.
- **Measured evidence wins over a documented default.** The compactor's `low`
  came from a benchmark. The memory tasks' tiers were measured on their
  recommended models. A provider default says nothing about those workloads.
- **A task's tier was chosen for its models, not for every model.** Title's
  `none` was chosen on Gemini 3 Flash. On Opus 5.5, which always thinks, it
  means something else.
- **One tier for every chat model was the bug.** A tier picked for Opus followed
  the user to Gemini.

## Consequences

- A workspace that stored a chat tier keeps it on the model it chats with: its
  `chat_model`, else the router's own. The engine moves it at boot, since SQL
  cannot see `LUCIDOS_MODEL`. Every other model now runs at its own default.
- A model whose provider documents no default sends no effort, and Claude and
  Gemini requests with no effort now ask for the provider's own default.
- A later seed migration must decide each new builtin's default effort. The
  test `every_builtin_declares_its_default_effort` fails until it does.
- A task with no recommended model reachable falls back to the chat model, and
  runs it at that model's default effort. The compactor's chat-model fallback
  does the same, where it once ran at `low`. On a thinking chat model these
  calls now think.
- `RoutingProvider` fills a missing effort from the default effort, so a call
  that names none follows the same rule.
- Coding agents are unchanged: Claude Code and Codex already apply their own
  default.

## Alternatives considered

- **Send no effort when unset.** Truest to "the provider's default", but the
  tier becomes unknowable to the engine. Rejected for the display and estimate
  reasons above. It remains the behaviour for a model with no documented
  default.
- **A per-model default replaces every task tier.** Simpler rule, but title,
  query classification and the command judge would think on every call on a
  model whose default is `medium` or `high`. Rejected: those tiers were chosen
  to spend nothing.
- **Store the chat tier on the model row**, beside `preferred_provider`.
  Rejected: the row describes the model, and a preference keeps the value
  reachable from the agent's `preferences` tool.
- **Keep one chat tier and reset it on model switch.** Loses the user's tier
  for the model they switch back to.
