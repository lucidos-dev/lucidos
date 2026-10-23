# 0544: The Lucidos Agent's rung locks while a turn runs; a pick between turns is a ChatSettingsChanged on the engine

- **Status**: Accepted
- **Date**: 2026-10-21

## Context

A Lucidos Agent thread's *rung control* took a pick while a turn ran. It then
named the picked pair, while the turn ran on to its end on the old one.

The pick itself lived in the tab's memory until the next send, which carried it
as an override. So a reload or another device dropped it. A turn the engine
started itself, for a child's report or a wake, ran the thread's recorded pair
while the pill named the pending pick.

ADR 0542 locks a coding-agent thread's rung while a turn runs, and records a
pick between turns on the engine. The user asked for the Lucidos Agent to match
it in full.

## Decision

The Lucidos Agent's rung locks while a turn runs, on the engine and in the
pill. A pick between turns is recorded on the engine at once, as a
`ChatSettingsChanged` thread event, through `POST /api/v1/chat/settings`.

- **A turn runs** while the thread's turn-start gate is held, or its status is
  `running`, `waiting_for_user_answer` or `paused`. These are ADR 0542's three
  statuses.
- **A locked pick is refused** with 409 and `reason: "turn-running"`, the slug
  ADR 0542 uses, and nothing is recorded. A mid-turn send that carries its own
  model, effort or provider gets the same 409, before anything is emitted. A
  send whose turn starts just after that check queues behind the turn instead
  of joining it, and runs next on its own pair.
- **The event records the resolved pair** (`resolve_chat_pair`), so it holds
  what the next turn runs on, never the raw request.
- **The next turn reads it**, however it starts: `last_thread_chat_settings`
  counts `ChatSettingsChanged` beside `MessageReceived` and `TriggerStarted`.
- **A turn-start gate** per thread orders a pick against a turn starting. A turn
  holds it from resolving its pair until its starter event is recorded. A pick
  takes it before it reads the status.
- **A message injected into a running turn** carries that turn's live pair. So
  its `MessageReceived` never names a model that did not answer it.
- **The browser holds no pending pick.** The pill reads the thread's events,
  and a follow-up send carries a pair only from a compose override.

## Rationale

A pick that means "from the next turn" has to live where the next turn reads
it. The engine starts turns the browser never sees, so only an engine record
reaches all of them. Every device and a reload then see the same pair.

Refusing a mid-turn pick keeps the pill honest. A pill that names a pair nothing
runs is a lie the user acts on: they judge the answer by the wrong model.

Recording the resolved pair makes the event say what runs. A raw request with
no effort would leave the effort to a later lookup, which may resolve
differently once a preference changes.

The gate closes the race between the check and the write. Without it, a pick
can read the status as idle just before a turn resolves its pair. That turn then
runs on the old pair while the event names the new one.

## Consequences

- One pill look for both agents: the shared lock plumbing is the same code.
- A pick between turns costs one HTTP call and one event, where it cost
  nothing before.
- `TURN_RUNNING_REASON` is generated into the SDK constants. ADR 0542's branch
  defines its own copy in `api/claude_code.rs`. Whichever lands second points
  both at one constant.
- Compose drafts are unchanged: their picks are per-draft overrides (ADR 0437),
  bound at the first send.

## Alternatives considered

- **Keep the pending pick in the browser, and only lock the pill.** It fixes the
  mid-turn lie but not the engine-started turn, and a reload still drops the
  pick.
- **Accept a mid-turn pick and apply it from the next turn.** The pill then has
  to name two pairs at once, the running one and the queued one. ADR 0542
  refused that shape for coding agents, and the two pills share one look.
- **Write the pick to the account preference.** It would change every new
  thread, which a per-thread control must never do.
