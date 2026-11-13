# 0608: Claude Code notes start hidden and the stream promotes them

- **Status**: Accepted
- **Date**: 2026-10-31

## Context

ADR 0605 decided from the Claude Code version whether a session's progress notes
reach the user. Three cases read as shown while no note arrives: an org with the
HIPAA policy, the beta switch in a settings file, and a provider that rejects
`thinking.display: "updates"`. Its follow-up was to decide from the stream.

A repro tried the obvious stream rule, "an empty `thinking` block means the
notes are hidden". Claude Code 2.1.311 with Opus 5.5, run as `claude -p
--output-format stream-json --verbose`, wrote prose before a tool call. It ran
twice: once with notes shown, once with `CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS=1`.
Both streams held a signature-only `thinking` block, then a `text` block, then
the `tool_use`. The `init` frame and its `capabilities` did not differ.

So an empty block proves nothing. The model also thinks without a note.

## Decision

A Claude Code session starts with its notes hidden, unless its spawn model is
known to think adaptively without always thinking. The first non-empty
`thinking` block from an always-thinking model proves the notes. The parser then
treats every later note as a `ProgressNote`, and the proof never reverts.

The agent hears each change. A resumed session on a model that starts hidden
reads a reset on its first input: notes count as hidden until one arrives. The
first note that arrives queues a promotion reminder in its place. Both ride the
next input that takes a prefix, once, as a `system-reminder`. A keep-alive nudge
and a slash command take none. An input the agent drops gives its reminder back.

## Rationale

The stream can only prove the notes shown, never hidden. Starting hidden is the
safe direction: the agent writes what the user must read in a message that calls
no tool, which shows in every case. A wrong "hidden" costs a stricter prompt
style. A wrong "shown" lost the answer.

The promoting note already streamed as a Thought through `thinking_delta`, so
its frame emits nothing. Otherwise the note would show twice. A sub-agent's
deltas do not stream, so its promoting frame still emits.

## Consequences

- No version check, no beta-switch read and no `claude --version` call decide
  this any more. This supersedes ADR 0605's version rule. Its removal of the
  Vertex relay stands.
- A spawn model the registry does not know, such as `claude-sonnet-4-6`, reads
  hidden. That costs only the stricter prompt style.
- The first note of a session shows as a Thought, not a `ProgressNote`. Later
  notes are `ProgressNote`.
- A mid-session switch from a model that starts shown to an always-thinking one
  keeps the shown sentence. The fallback-model gap is logged in
  `docs/temporary-measures.md`.

## Alternatives considered

- **An empty block means hidden.** Rejected by the repro above: the block is
  empty with notes shown too.
- **Read the display mode from the `init` frame.** It does not carry it. The
  `capabilities` were identical in both runs.
- **Keep the version rule.** It cannot see the HIPAA org, the settings-file
  switch or a rejecting provider.
- **Ask the user.** The engine can learn the answer from the stream for free.

Non-goals: changing the Codex path, and changing how the chat round quotes a
note.
