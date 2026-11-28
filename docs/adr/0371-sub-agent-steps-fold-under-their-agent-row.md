# 0371: A sub-agent's steps fold under its agent row, grouped by the persisted parent link, never by timing

- **Status**: Accepted
- **Date**: 2026-10-06

## Context

A Claude Code session that calls its `Agent` tool streams the sub-agent's tool
calls on its own stdout. The transcript drew them as flat siblings of the
session's own steps, each with its own context counter. Two sub-agents running
at once interleaved, and nothing said which agent took which step.

The same gap misplaced counters. A coding-agent capture bound to whichever step
was last, so with parallel agents a counter landed on another agent's row.

Claude Code tags every line a sub-agent streams with `parent_tool_use_id`, the
id of the `Agent` call that spawned it. The parser read that tag only for usage
accounting, then dropped it. Plan:
[`docs/plans/2026-10-06-fold-sub-agent-steps-under-agent-row.md`](../plans/2026-10-06-fold-sub-agent-steps-under-agent-row.md).

## Decision

The engine keeps `parent_tool_use_id` on `CodingAgentToolCalled`,
`CodingAgentToolResult`, `ContextCaptured` and `CodingAgentTextStreamed`. The transcript nests each
sub-agent step under its `Agent` row, folded, with the running agent's latest
step on one line beneath it.

## Rationale

- **Persisted, because timing cannot tell agents apart.** The rows between an
  `Agent` call and its result belong to that agent only when one runs at a time.
  Parallel and background agents break it.
- **Folded with a live tail.** The user picked it from three rendered options.
  The parent's steps stay scannable, and a running agent still shows progress.
- **Captures bind by agent.** The same field fixes the misplaced counters, in
  both step projections.
- **A group sits at its agent row** (ADR 0201). Children keep clock order inside
  it, and the parent's rows keep theirs around it.

## Consequences

- Rows written before the field carry no parent and render flat, as before.
- A sub-agent's prose is recorded with its tag and kept out of the parent's
  reply. About one `Agent` run in twenty narrates, and that text used to read as
  the parent's own. Older rows carry no tag and still read that way.
- Fold state persists per agent call in localStorage, like the turn folds.
- Codex has no sub-agents, so its steps never carry a parent.
- The STALE_RESUME recap and the summary tree log still read sub-agent calls as
  the parent's.

## Alternatives considered

- **Infer the group from timing on the frontend.** No engine change, but it
  misgroups as soon as two agents overlap. Rejected.
- **Nested and always open** (option A). Keeps every step visible, but a long
  sub-agent buries the parent's own steps. The user picked the fold.
- **Fold that opens while running** (option B). Shows the full list during the
  run, which is when it is longest and least useful. The user picked the tail.
