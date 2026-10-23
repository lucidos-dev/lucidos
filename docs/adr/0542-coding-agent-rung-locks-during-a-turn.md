# 0542: A coding-agent thread's model and effort lock while a turn runs; a pick between turns is recorded at once

- **Status**: Accepted
- **Date**: 2026-10-21

## Context

A coding-agent thread's rung (model and reasoning effort) could change while
a turn ran. Each backend took it differently:

- Claude Code applied a model pick to the running turn (`set_model`).
- Claude Code takes effort only at spawn, so an effort pick interrupted the
  turn and resumed it at the new effort. That took a stop reason, a cancel
  cause, a resume message, and a ledger of inputs the restart discarded.
- Codex applied both at its next turn.

Between turns, a pick on a started thread lived only in a browser signal until
the next follow-up carried it. A reload, a focus change or another device lost
it. The "Answered by" popover read the previous exchange's rung until the new
session reported its own, because a coding-agent `MessageReceived` carried no
model.

## Decision

A turn runs while it starts, works, or waits on a question. While it runs, the
engine refuses a model or effort pick with 409 and `reason: "turn-running"`,
and the picker is disabled. Between turns, a pick is recorded at once as
`CodingAgentSettingsChanged` with the user's actor, and the next spawn reads
it. A coding-agent `MessageReceived` carries the model and effort its turn
runs on.

## Rationale

A mid-turn change is rare, and every mid-turn path was a different contract
with its own failure modes. A turn interrupted for an effort pick is the worst
of them: it spends a cache rebuild and a resume to change a setting the user
could have changed a minute later for free. Locking the rung for the turn
removes all three paths and makes both backends behave the same.

Recording the pick on the server makes it a preference like any other: it
survives a reload and shows on every device (CLAUDE.md § Engine
Statelessness).

## Consequences

- Live `set_model`, the effort restart and Codex's next-turn model path are
  gone. `ControlRequest` carries only `Interrupt`.
- Persisted events from before still name `CancelCause::EffortRestart` and the
  `effort_restart` continuation reason. Both still deserialise and render.
- The warm-cache dialog (hand off, keep, or switch) still runs before a pick
  between turns.
- A user who wants a new rung mid-turn stops the turn, or waits for it.

## Alternatives considered

- **Lock the rung for the thread's life**, and switch only by a handoff.
  Rejected: on a cold cache an in-place switch is free and keeps the whole
  conversation, while a successor keeps only a narrative.
- **Fix only the popover**, keeping every mid-turn path. Rejected: it leaves
  three contracts and the lost browser pick in place.
- **Keep the pending pick in the browser** until the next send. Rejected: it
  is lost on a reload and invisible to other devices.
