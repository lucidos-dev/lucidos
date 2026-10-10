# 0323: A queued Claude Code message can be taken back until the agent reads it, through cancel_async_message

- **Status**: Accepted
- **Date**: 2026-09-29

## Context

A follow-up sent to a running Claude Code session goes to its stdin at once.
That write is what makes mid-turn steering work: Claude Code folds the message
into the running turn at its next tool result. Until the agent reads it, the
transcript shows it as "Queued" (ADR 0268).

The first design drew no bin on such a message. The engine had already written
it, so the design took it as beyond recall. The user called that ridiculous.
They also asked for Edit, on this lane and on the Lucidos Agent's.

Claude Code's stream-json control protocol has a request for exactly this:
`{"subtype":"cancel_async_message","message_uuid":…}`. It drops a user message
still waiting in Claude Code's command queue and answers `cancelled: true`. Once
the message is read into a turn, dequeued or folded, it answers `false`.

## Decision

A queued Claude Code message offers a bin and Edit until the agent reads it. The
engine asks Claude Code to drop it with `cancel_async_message`, and records the
`QueuedMessageRemoved` tombstone only when Claude Code answers `cancelled: true`.
Every other answer is a refusal the user sees. Codex has no equivalent, so its
queued messages offer neither.

Edit is the same take-back, followed by putting the message's text and images in
the compose box. It exists on both lanes.

## Rationale

- **Steering and withdrawal stop being a trade.** Holding follow-ups in the
  engine would make them withdrawable, and would lose mid-turn steering. The
  control request keeps the immediate write and still takes the message back.
- **Claude Code decides, so the tombstone cannot lie.** The boundary it answers
  on is the read, the same line the "Queued" label already follows. A tombstone
  recorded on any weaker signal would hide a message the agent then answers.
- **The run loop records the tombstone, not the HTTP handler.** The loop owns
  the input ledger and hears Claude Code's answer. Recording there keeps the
  ledger, the tombstone and Claude Code in step even when the request times out.

## Consequences

- Every stdin user line now carries a `uuid`, fresh per write. Claude Code skips
  a user message whose uuid it has already seen, and acknowledges it as read.
  So a reused event id would silently drop a message the engine resends. The
  input ledger maps each `MessageReceived` to the uuid its write carried.
- A withdraw must never reach Claude Code ahead of the message it names. The
  chat fast path records a follow-up as unforwarded before its send. The run
  loop holds a withdraw for it until it forwards the message, and the driver
  writes every pending input before a cancel line.
- A write that coalesced several messages (a spawn's first prompt) cannot be
  taken back one message at a time, so it is refused.
- The feature leans on a Claude Code control request the Lucidos code does not
  own. An older CLI answers it with an error, which reaches the user as a
  refusal, never as a false removal.
- Stop still leaves a Claude Code thread's queued messages with the agent, as
  Claude Code's plain interrupt does. Only the bin or Edit takes one back.

## Alternatives considered

- **Hold follow-ups in the engine until the turn ends.** Withdrawal becomes a
  local delete, but the agent can no longer be steered mid-turn. The user chose
  steering when the queue was first drawn, and the control request makes giving
  it up unnecessary.
- **Keep no bin.** The original choice. It rested on a premise, that a written
  message cannot be recalled, which Claude Code's protocol disproves.
- **Name the stdin message by its `MessageReceived` event id.** One id fewer to
  track, but Claude Code dedupes user messages by uuid across a resumed session,
  so a resent follow-up would vanish.
- **Interrupt with `cancel_queued: true`.** It sweeps every queued message and
  aborts the running turn, which is Stop, not taking back one message.
