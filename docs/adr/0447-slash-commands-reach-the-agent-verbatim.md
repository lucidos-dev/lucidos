# 0447: A slash command reaches the coding agent verbatim, and its read moves no turn-gap bound

- **Status**: Accepted
- **Date**: 2026-10-12

## Context

A slash command sent from the composer ran as a model prompt in a Claude Code
session. The engine puts engine context in front of every text it forwards:

- on a spawn's first prompt, the resume-time notes and the device and
  preferences block;
- on a live follow-up, the turn-gap note and the device block.

Claude Code runs a command only when the input starts with `/`. We probed
Claude Code 2.1.280 with the engine's flags. Behind the device block, `/cost`
ran a model turn, and the model answered "I don't have a /cost skill". Sent
bare, it answered locally with `num_turns` 0.

A bare command raises a second question. The turn gap counts a boundary once the
agent reports reading the input that carried it. A bare command carries no
note, but its read is the agent's own report, so it would count. The next
prompt would then lose every event from before the command.

## Decision

An input whose text starts with `/` is a *verbatim input*: the engine forwards
it exactly as typed, on a spawn and on a live follow-up.
`input_ledger::forwards_verbatim` is the one definition. Its
`CodingAgentInputRead` carries `verbatim: true`, which the ledger derives from
the text it recorded. The turn-gap boundary query ignores such a read, exactly
as it ignores an `assumed` one.

## Rationale

- **The read is the right place for the flag.** Every turn-gap boundary already
  needs a read to count. A verbatim input writes up to two boundary rows: its
  `MessageReceived`, and on a follow-up its `CodingAgentPromptSent`. The read
  names both, so one filter covers the spawn path and the follow-up path.
- **The flag comes from what was sent.** The ledger keeps the text it handed
  the agent. That text starts with `/` exactly when it went verbatim. So the
  flag cannot disagree with the delivery, and no forward takes a new parameter.
- **The rule is by text, not by backend.** Engine prompts never start with `/`.
  A Codex input that does loses the device block for one turn. Its turn-gap
  note moves to the next framed prompt rather than being lost.
- **The stale-resume retry stays framed.** It puts a reconstruction of the
  conversation in front, so a command sent first after a stale resume runs as a
  prompt once. Sending it bare would start the fresh session with no history
  for every later prompt. A loud, retryable failure beats a silent loss of
  context.

## Consequences

- Slash commands from the composer, and typed ones, run as commands.
- The next framed prompt carries the turn gap from before and after a command.
- A slash command leads a spawn alone. A message typed during the spawn's
  startup queues for the run as its own input (ADR 0442). So it never becomes
  the command's arguments.
- A command sent as the first message of a respawn skips three resume-only
  facts. Each needs a durable record of an undelivered note, which is a
  separate design:
  - commits made outside the agent (uncommitted edits are still reported);
  - a branch adoption in an external repo;
  - a restart that cut off the previous turn.

## Alternatives considered

- **Append the notes after the command.** Claude Code passes the rest of the
  line to the command as arguments, so `/eli5 tcp` and `/compact <focus>` would
  receive engine text.
- **Send the notes as a separate input first.** Each one opens a model turn the
  user did not ask for, and the model answers the context block.
- **Classify by text in the turn-gap SQL.** A second definition of the
  predicate, in another language, over a stored text that a coalesced or
  reconstructed input does not hold verbatim.
- **Write no `CodingAgentPromptSent` for a verbatim input.** The spawn path
  writes none anyway, and its `MessageReceived` is a boundary too. It would
  cover half the cases and lose the audit record.
- **Classify commands from Claude Code's own command list.** The init event's
  `slash_commands` omits `/cost` and mixes local commands with prompt
  commands.
