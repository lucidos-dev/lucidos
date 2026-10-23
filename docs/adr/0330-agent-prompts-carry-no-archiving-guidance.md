# 0330: Agent prompts and knowhow carry no guidance on when to archive; archive is a discoverable tool action only

- **Status**: Accepted
- **Date**: 2026-09-29
- **Amends**: [ADR 0310](0310-agents-decide-when-to-archive.md), its "The judgement lives in the guidance" bullet

## Context

ADR 0310 gave agents an archive action and put the judgement of when to use it
in the guidance. Several surfaces carried that guidance: the chat system
prompt, every coding-agent prompt that can spawn a sub-thread, two knowhow
files and the glossary. Each told an agent to archive a thread once its change
was applied and no follow-up was expected.

The maintainer wants archiving to be a tool action an agent can find, and
nothing more. Agents keep the ability to decide, but the engine does not
encourage them to use it.

## Decision

No prompt, tool description or knowhow file tells an agent when to archive a
thread. The `threads` tool's `archive` action and `lucidos threads archive`
stay, described neutrally: what they archive, and which states refuse them.

## Rationale

**When to archive is the user's decision.** An agent may still archive, but
nothing should encourage it to. A standing rule in every prompt did exactly
that, and cost bytes on every turn of every session.

**The user already has two places to delegate it.** A user who always wants
finished threads archived says so in the user profile. A user who wants it for
one piece of work says so in the thread that starts it. That thread's agent
archives its own children, and the cascade takes their sub-threads with them. Both
are the user's own words, so the choice stays theirs.

## Consequences

- The coding-agent prompts lose `ARCHIVING_THREADS_RULE`, and the chat prompt
  loses its PARALLEL WORK bullet on archiving.
- The knowhow keeps the action's mechanics and refusals, and the fact that
  nothing archives a thread on its own.
- Applied children stay in the drawer until someone archives them.

## Alternatives considered

- **Replace the rule with "archive only when asked".** Rejected: it is still
  guidance on when to archive, just inverted.
- **Keep the rule in knowhow only, out of the prompts.** Rejected: an agent
  that loads the knowhow meets the same instruction.
