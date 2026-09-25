# 0268: A coding-agent input is owed until the agent reads it: Claude Code replays it, Codex starts its turn; a Result settles nothing

- **Status**: Accepted
- **Date**: 2026-09-24

## Context

A coding-agent session exits its subprocess at idle unless the agent still owes
a forwarded input. The engine decided what was owed at each `Result`. For
Claude Code it assumed one `Result` answers every input forwarded so far.

That holds only for part of a turn. Claude Code folds a stdin input into the
running turn at the next tool result. An input that arrives after the turn's
last tool call is not folded in. Claude Code ends the turn with its `Result`,
then runs the input as a second turn. The engine zeroed the debt at the first
`Result` and killed the subprocess, so the second turn never ran.

A held message (ADR 0256) hit it first. The human answered the question in band,
the release forwarded the parent's message right after the answer's tool
result, and the agent never saw it. The transcript showed it as delivered. Any
input in the same window is lost the same way.

## Decision

A forwarded input stays owed until the agent reports it read. Claude Code runs
with `--replay-user-messages` and reports a read by replaying the input. Codex
reports one when its driver starts the turn that carries the input. A `Result`
settles nothing on either backend.

Each read emits `CodingAgentInputRead`, naming the event that carried the
input. The transcript marks a message "Sent" until then, and "Read" after. The
rule lives in `engine/agent_session/input_ledger.rs`.

## Rationale

**The replay is exact.** Probes against the real CLI showed Claude Code replays
an input at the moment it consumes it: mid-turn after a tool result when it
folds the input in, and at the next turn's start when it does not. Guessing from
the order of events, which the old rule did, cannot tell those apart.

**One replay can carry several inputs.** Inputs that queue while a turn runs
start the next turn together, behind a single replay. Plain inputs come back
joined into one text with newlines. With an image among them, each input's
blocks come back apart.

So the engine keeps what it sent each input, and matches a replay against the
oldest owed inputs. Every input the replay carries is read at once. A replay
that matches no run of inputs, such as `/compact` output, reads the oldest one.

**A read is the one thing the user wants to know.** The old transcript said
"delivered" when the engine wrote to stdin. That is exactly what lied here.

**Local commands get a grace, not a special case.** `/cost` runs a turn with no
replay. When a `Result` leaves inputs owed and the agent stays silent for a short
grace, the engine marks them read and exits. A queued turn starts within
milliseconds of the `Result`, so the grace never fires on a real input.

## Consequences

- A message that lands after the agent's last tool call is read as the next
  turn, never dropped. That covers held messages, the human's message after a
  Cancel, and ordinary follow-ups.
- The inputs Claude Code merges into one turn still settle in that turn, so a
  merged turn exits at idle as before.
- A continuation turn runs with no forwarded input, and settles nothing.
- History from before this change has no read events, so its messages carry no
  marker.

## Alternatives considered

**Make the held-message release wait for the turn to end.** Fixes the reported
repro only. A second held message, the human's message after a Cancel, and any
ordinary follow-up in the same window would still be lost.

**Keep one input owed per `Result` for Claude Code too.** Right for the second
turn, wrong for a merged turn. The merged inputs would stay owed, and the
subprocess would stay up after the work is done.

**Read one input per replay.** The first version did. When two inputs shared a
replay, the second stayed owed until the silent grace settled it, after the
turn had ended. The transcript then showed it queued for ten seconds, and the
late read opened a turn for it that never came.

**Let a turn with no replay settle one input.** Handles `/cost` without a timer.
Rejected: a continuation turn also has no replay, and it would settle a
follow-up the agent has not read.
