# 0268: A coding-agent input is owed until the agent reads it: Claude Code replays it, Codex starts its turn; a Result settles nothing

- **Status**: Accepted
- **Date**: 2026-09-24
- **Amended**: 2026-10-12. A read names its input by uuid, and the silent
  grace is gone (`docs/plans/2026-10-12-coding-agent-reads-name-their-input.md`).

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

Every read names its input by the uuid the engine wrote it under
(`AgentInput::uuid`), and settles that input alone. A Codex turn that never
read its input names it as rejected. It leaves the ledger unread, and its
`Result` carries the error. Nothing else settles an input: no timer, and no
guess from the order of events.

Each read emits `CodingAgentInputRead`, naming the event that carried the
input. The transcript marks a message "Sent" until then, and "Read" after. The
rule lives in `engine/agent_session/input_ledger.rs`.

A read that opens a turn after the last one ended carries `started_turn`. It is
that turn's start event, because the input's own `CodingAgentPromptSent` landed
before the terminal that ended the old turn. Without it, a queued message that
outlives a Stop ran on a thread still marked idle and stopped.

## Rationale

**The replay is exact.** Probes against the real CLI showed Claude Code replays
an input at the moment it consumes it: mid-turn after a tool result when it
folds the input in, and at the next turn's start when it does not. Guessing from
the order of events, which the old rule did, cannot tell those apart.

**The replay names its input.** Probes against Claude Code 2.1.280 showed a
replay under the input's own uuid for every shape: a plain prompt, a local
command, a compaction, inputs folded into a running turn, and inputs queued
behind one. Queued inputs each replay alone, then the last one's uuid replays
all their texts joined. `/compact` also replays its stdout under a uuid no
input has, which reads nothing.

The first version matched replay texts against the oldest owed inputs. That
read `/compact`'s stdout as the compaction, then the compaction's own replay as
the follow-up queued behind it. Matching by uuid has no such case.

**A read is the one thing the user wants to know.** The old transcript said
"delivered" when the engine wrote to stdin. That is exactly what lied here.

**Local commands need nothing special.** `/cost` answers with no model turn,
and Claude Code still replays it under its uuid. So every input is read. An
idle that owes one keeps its subprocess until the read comes, however long a
`UserPromptSubmit` hook or a compaction takes.

## Consequences

- A message that lands after the agent's last tool call is read as the next
  turn, never dropped. That covers held messages, the human's message after a
  Cancel, and ordinary follow-ups.
- The inputs Claude Code merges into one turn still settle in that turn, so a
  merged turn exits at idle as before.
- A continuation turn runs with no forwarded input, and settles nothing.
- A caller that waits on its own prompt gets a completion with it. Apply's
  harden and merge prompts use one. The read moves it to the turn that read the
  prompt, and only that turn's end answers it. A turn already running when the
  prompt landed cannot, so Apply never reads the harden marker early.
- History from before this change has no read events, so its messages carry no
  marker.

## Alternatives considered

**Make the held-message release wait for the turn to end.** Fixes the reported
repro only. A second held message, the human's message after a Cancel, and any
ordinary follow-up in the same window would still be lost.

**Keep one input owed per `Result` for Claude Code too.** Right for the second
turn, wrong for a merged turn. The merged inputs would stay owed, and the
subprocess would stay up after the work is done.

**Match replay texts against the oldest owed inputs.** The first version did.
It needed a fallback for a replay matching nothing, which read the oldest input.
`/compact`'s stdout replay then read a queued follow-up the agent had not taken.

**Settle owed inputs after a silent grace.** The engine once did this when the
agent stayed quiet for ten seconds at idle: it marked inputs read and killed the
subprocess. It meant to cover local commands it believed never replay. They do replay. The grace instead
fired on a real input whose read a slow hook delayed, and nothing sent it again.

**Classify local commands at forward time, so they are never owed.** No list
can do it. `/cost` is missing from Claude Code's own `slash_commands`, and that
list mixes local commands with prompt commands. It is also unnecessary, since
the replay already reports them.

**Let a turn with no replay settle one input.** Handles `/cost` without a timer.
Rejected: a continuation turn also has no replay, and it would settle a
follow-up the agent has not read.
