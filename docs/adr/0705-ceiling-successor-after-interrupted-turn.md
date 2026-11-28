# 0705: Start ceiling successors after an interrupted turn

- **Status**: Accepted
- **Date**: 2026-11-15
- **Amends**: [0629](0629-engine-enforced-context-ceiling.md)

## Context

ADR 0629 makes ceiling successors wait for input. ADR 0701 also delivers
ceiling re-entry mid-turn. That can interrupt unfinished work with no future
input to resume it.

## Decision

An agent's narrative handoff starts after its turn when ceiling re-entry
interrupted a running turn. A natural turn end keeps the next-input start.

Read existing events in the current session: take the latest
`ContextCeilingReached` and its engine `context_ceiling` `PromptInjected`.
No turn-end event between them means the turn was interrupted.
Use the shared turn-end event list, covering both coding-agent backends.
Missing evidence or a failed read keeps the next-input start.

## Rationale

A running turn has unfinished work. Its successor should continue from the
narrative without making the user type Continue. A naturally ended turn
already waits for input, so starting it would create work without a request.

## Consequences

The engine's brief still waits for input, preserving Stop. A typed Successor
card request still starts immediately. A card tap without a request waits.
No event, field or agent flag is added. The rule applies to every workspace
and both backends; drivers without steering naturally retain the waiting path.

## Alternatives considered

- Always wait: strands interrupted work.
- Always start: wakes naturally ended turns without a request.
- Use `CodingAgentInputRead.started_turn`: false for both delivery paths, so it
  cannot distinguish them.
- Add a delivery field: existing event order already supplies the fact.
