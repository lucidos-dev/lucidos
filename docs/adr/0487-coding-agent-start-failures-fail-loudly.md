# 0487: A coding agent that exits on its own fails the turn with its own stderr, and a start failure is never retried automatically

- **Status**: Accepted
- **Date**: 2026-10-15

## Context

A user lost the network while two coding-agent threads were mid-turn. The hang
watchdog retried each thread once, which was right: the session really had
hung. The retried `claude` process then exited 1 within seconds, before its
first startup line, and printed why on stderr.

Lucidos threw the exit code and the stderr away. The safety net saw only "no
terminal event" and wrote `ResponseAborted { cause: safety_net }`. The thread
read "Response interrupted, looked stuck", with an empty Details fold. Every
later Continue repeated the same silent abort. Codex had the same hole when its
app-server child died before the handshake. The plan is
`docs/plans/2026-10-15-coding-agent-start-failures-fail-loudly.md`.

## Decision

**A coding agent whose process exits on its own fails the turn with the CLI's
own words.** A start failure is never retried automatically.

- Each driver reads stderr while the child runs and keeps a masked 4 KB tail.
  The exit code and the tail travel on `AgentEvent::Exited` to the one decision
  point, `safety_net_action` in `lifecycle.rs`.
- An exit before Claude Code's first `system/init` line, or before the Codex
  app-server handshake, is a **start failure**. The classifier reads timing
  only, never the wording of an error.
- A non-zero exit, or any stderr, at either stage becomes `ResponseFailed`. It
  carries the CLI's words in `error` and an optional
  `agent_exit { stage, code }`. Old events have no `agent_exit` and render as
  before.
- A watchdog kill and a stray signal kill keep their single continuation.
- The failure card quotes the CLI, says why Lucidos did not retry, and offers
  **Try again**. That is the existing Continue endpoint, with no new route and
  no new reason.
- Try again resends the user's message the agent never read, instead of a bare
  continue message (ADR 0268: an input is owed until the agent reads it).

## Rationale

A start failure repeats until a person fixes its cause, so an automatic retry
only delays the explanation. The CLI is the authority on its own login and
network state, so showing its words beats any probe Lucidos could write.
Timing is the one signal that survives CLI releases. Error wording does not.

`ResponseFailed` already drives the `failed` projection and the red card, so
the change adds one optional field and no terminal variant.

`AgentStartFailure` is a typed error. The first prompt cannot be sent when the
driver has already exited, and that race happens before the run loop can emit
anything. Two callers settle such a turn. Each must tell it from any other send
error to attach `agent_exit`. So it joins the blessed error types in
`.claude/rules/rust.md`.

## Consequences

- A thread whose agent cannot start ends `failed` with a readable reason, where
  it used to end `interrupted` with none.
- Stored stderr is bounded and masked. Injected secret values and Postgres
  passwords never reach an event.
- Exit 0 with empty stderr and no result keeps the old `safety_net` abort.
- A thread parked on a start failure waits for a person. Restart recovery does
  not resume a `failed` thread.
- No push notification fires for a failed thread. That stays a separate
  decision, because it touches every failure kind.

## Alternatives considered

- **Retry once, then fail.** A start failure repeats, so the second attempt
  costs time and adds a misleading "Resumed" row. Rejected.
- **Classify by stderr wording** ("auth", "network"). The text changes with
  every CLI release, and a missed pattern falls back to silence. Rejected for
  timing.
- **A pre-flight login and network probe.** It duplicates what the CLI knows
  and can disagree with it. Rejected.
- **A new `ResponseStartFailed` event.** Every projection and card would learn a
  second failure terminal for no gain over one optional field. Rejected.
- **Show the CLI words in `ResponseAborted`.** An abort means the system ended
  the turn, and it has no message field. Rejected.
