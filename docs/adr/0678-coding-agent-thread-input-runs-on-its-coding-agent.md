# 0678: A coding-agent thread's input runs on its coding agent, chosen by the thread's source; an unknown lane fails closed

- **Status**: Accepted
- **Date**: 2026-11-13

## Context

A coding-agent run can end with follow-ups still in its channel. The engine
drains them and hands them to the orphan chain (`api::chat::process_orphan_chain`).
The chain re-ran every batch on the Lucidos Agent, because it called the turn
entry with `use_coding_agent: None`.

On a coding-agent thread that meant a chat turn with the coding agent's model.
An install with no chat route for that model got a routing error naming the
model's first route, which read as "Claude Code tried Vertex". An install with
a route got worse: the Lucidos Agent answered silently in the coding-agent
thread, spent a chat credential, and the coding agent never saw the message.

Plan: `docs/plans/2026-11-13-coding-agent-input-never-runs-on-the-lucidos-agent.md`.

## Decision

The sent thread decides which agent runs an input, never the caller. A thread whose
`thread_summaries.source` is `claude_code` runs its coding agent for every
input. The one lane read is `engine::thread_agent`. When it cannot answer, the
input is refused with a visible routing failure, and no turn runs.

A composing draft keeps the first send's requested mode. Its source reflects
the last saved compose toggle, which can lag behind the mode the user sends.

## Rationale

- The first message locks `source`; compose toggles can change it before that.
  The switch resume and orphan admission already select by it.
- A caller-supplied flag is the bug: one caller among many passed `None`, and
  `None` meant the chat lane. The thread's own row has no such caller to forget.
- Failing closed costs a retry. Failing open costs a chat credential and sends
  the coding-agent transcript to a chat provider the user never chose for it.

## Consequences

- Every orphan batch on a coding-agent thread runs through the coding-agent
  path, each message as itself, so the run writes its `CodingAgentPromptSent`.
- Unread Continues keep their text and event anchor as engine re-entry. They
  can carry answer recaps or hardening requests, even when a run never started.
  A redundant bare Continue may cause an extra coding-agent turn; preserving
  instructions takes priority over suppressing it. No text heuristics apply.
- A lane read error leaves the message unanswered, with a `ResponseFailed`
  saying why. The user can send it again.
- ADR 0506 still covers inputs a dead process held. This ADR covers inputs a
  live run left in the engine's channel.

## Alternatives considered

- **Trust each caller's `use_coding_agent`.** That is what broke. Every new
  caller would have to remember it, and the default is the dangerous lane.
- **Read `is_coding_agent`.** It drifts: a workspace held 527 `claude_code`
  threads with it false, after an old projection bug relabelled them.
- **Fall back to the chat lane when the row cannot be read.** It reproduces the
  incident under a database hiccup instead of a missing flag.
