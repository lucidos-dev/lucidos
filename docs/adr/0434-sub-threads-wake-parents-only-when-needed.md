# 0434: A sub-thread's clean report wakes its parent only when the child says the parent must act

- **Status**: Accepted
- **Date**: 2026-10-11
- **Amends**: [0011](0011-parent-child-fan-in-durability.md)
- **Amended by**:
  - [0435](0435-wake-decision-shares-the-read-decisions-default.md), which runs an unset wake decision on OpenAI Decisions
  - [0446](0446-read-decision-knows-the-parent-acts.md), which settles the wake decision before a forced read decision that reads it
  - [0451](0451-spawn-declares-the-wake-mode.md), where the spawn declares a wake mode and this decision becomes the fallback

## Context

Every terminal turn of a direct child sent a `ChildThreadCompleted` and re-ran
its parent. For a parent that combines results or takes a next step, that is
the point. For the *home thread*, it was mostly noise: Home starts threads for
work so the work runs in parallel (ADR 0362, 0411). The child's findings reach
the user in the child's own thread. Home woke only to relay them, spending a
turn and pushing its own conversation down.

The read decision (ADR 0417) already had the shape we wanted: the agent decides
at its turn end, and a turn that skips it is decided by one background call.

Plan: `docs/plans/2026-10-11-sub-threads-wake-parents-only-when-needed.md`.

## Decision

Every child turn makes a *wake decision*: does the parent have to act on this
report now? A yes, an undecided turn, and every finish that is not clean wake
the parent with a `ChildThreadCompleted`, as before. A no on a `success` or
`no_changes` finish files the report as a `ChildReportFiled`, which starts no
turn. The parent reads it on its next turn.

- **The child decides** with `wake_parent { wake }` (offered only to a thread
  with a parent) or `lucidos wake-parent yes|no`
  (`POST /api/v1/threads/:id/wake-decision`). It lands as
  `ParentWakeRequested` or `ParentWakeNotRequested` on the child, projected to
  `thread_summaries.parent_wake_requested`. Every start event resets it to
  NULL in the statement that sets `parent_callback_pending`, and the report it
  governed spends it. So a turn with no start event of its own, such as a chat
  child woken by its own child's card, reads undecided.
- **A turn that skips it** is decided at the turn-end gate by one call to
  `JudgmentSite::WakeDecision`, beside the read decision. It reads the message
  that opened the child's turn, the parent's latest reply, the child's reply,
  its finish and how many siblings still run. It stays on its chat model
  (`model_wake_decision`) until the user picks a System One row
  (`judgment_wake_decision`), per ADR 0363.
- **Every doubt wakes.** That covers a failed or timed-out call, a missing
  answer, an answer at or above the threshold and an empty reply. It covers a
  turn the engine cut short and an unreadable row too.
- **A filed report is its own event type**, with the same payload. Its
  projection settles the child's `parent_callback_pending`, and writes nothing
  on the parent.
- **A chat parent** reads it as a `[CHILD THREAD REPORT FILED]` block in its
  history. **A coding-agent parent** reads it whole in the turn-gap note at the
  front of its next prompt, once.

## Rationale

**A new event type, not a flag on `ChildThreadCompleted`.** At least eight
readers classify `ChildThreadCompleted` as a turn start: `thread_lifecycle`,
chat and agent recovery, held deliveries, event waits, worktree cleanup,
`resume.rs` and the boot refire. A flag would have to be honoured in each, and
one that missed it would wake the parent or re-fire the card on boot. A new type
is invisible to all of them, so a filed report cannot start a turn by
construction.

**The child decides, not the parent.** Only the child knows whether its result
is a step in someone's plan or a finding for the user. The parent's need is
written down where the child reads it: the spawn message. The background call
also reads the parent's latest reply, where a parent says what it waits for.

**Only a clean finish can file.** A failure, a cancel, an `interrupted` turn and
a short-cut turn mean the plan broke, which is exactly when the parent must act.
They wake whatever the child said.

**ADR 0011's durability holds.** The boot refire selects only
`ChildThreadCompleted`, so a filed report is never re-fired. It is also no
reaction: a filed report after an unprocessed card leaves that card the
parent's last word, and the card's lost wake is still re-driven.

## Consequences

- A parent that needs a child's result must say so in the spawn message or the
  follow-up. The chat system prompt, the spawn tools' descriptions, the Codex
  CLI rule and the shipped knowhow all say so.
- A child turn that skips the decision costs one more background call, run
  concurrently with the read decision.
- An `await_event` on `ChildThreadCompleted` for another thread's child misses a
  filed report. The tool description and the knowhow say to name
  `ChildReportFiled` beside it.
- A coding-agent parent reads filed reports through the turn-gap note. A
  follow-up to a kept-alive session carries that note too, ending at the
  follow-up's `CodingAgentPromptSent`. That boundary counts only once the
  agent's `CodingAgentInputRead` names it. So a report rides a prompt the agent
  read, never one lost to a failed send, a dead agent or a withdrawal. The
  bounds and the argument are in
  `docs/plans/2026-10-11-turn-gap-note-for-kept-alive-sessions.md`.
- A held card, sent when a waiting child's last event wait ends, always wakes:
  its turn ended holding a wait and decided nothing to file on.
- A follow-up that reaches a child mid-turn is a start event, so it reopens a
  decision the child already made. The turn's report changed, and the agent's
  acknowledgement says so.

## Alternatives considered

- **A `filed: bool` on `ChildThreadCompleted`.** Rejected for the eight readers
  above, each a place a missed check would wake a parent or re-fire a card.
- **The parent decides, at spawn time, which children may wake it.** Rejected:
  the parent cannot know at spawn time whether a result will need action, and a
  failure must wake it whatever it said.
- **Default to filing unless the child asks to wake.** Rejected: a silent
  default that hides a report from a parent waiting on it stalls the plan with
  no error. Every doubt wakes instead.
- **One combined read-and-wake decision.** Rejected: they answer different
  questions for different readers. A report the user should read may need no
  parent action, and the reverse.
