# 0349: Thread triage and Archive all share one safety classifier; triage applies only after the user replies; Archive all skips what needs the user and carries Undo

- **Status**: Accepted
- **Date**: 2026-10-02

## Context

A tester asked for an "Archive All" button. They had a pile of threads and 18
open questions, mostly stale runs of monitoring triggers. A design grill first
chose an agent-side triage and no button. On review the maintainer added the
button back, with a safety net. Both need the same answer to one question:
which threads are safe to put away?

Three facts shaped the design:

- An agent may archive only its own thread or a direct child (ADR 0310).
- Delete is owner-only, hard and not recoverable inside Lucidos (ADR 0192).
- Archive had no undo. Nothing moved an archived thread back on request.

`docs/plans/2026-10-02-thread-triage.md` makes this executable.

## Decision

**One classifier, `engine::thread_triage`, decides from facts which inbox root
threads need the user.** A thread needs the user when it, or a sub-thread, has
any of these:

- an unanswered question;
- a pending change, or an unproposed branch diff;
- an unsent draft;
- a failed last run, a running turn, or a live event wait.

The title is never an input.

**Thread triage** is two actions on the `threads` tool:

- `triage` proposes an action per root, with a reason, and records a
  `ThreadTriageProposed` event on the calling thread.
- `apply_triage` applies entries only after the user replied to that proposal
  in the same thread. A reply is a `Device`-actor `UserQuestionAnswered`, or
  a `Device`-actor `MessageReceived` the agent has read, newer than the
  proposal. A queued follow-up the agent has not read, or one the user
  withdrew, is no reply. Every entry must name a thread in the proposal, and
  every entry is re-checked against fresh facts.

The applied verbs are `archive`, `pin` and `dismiss_question`. A thread that
needs the user is never archived. `delete` is proposed but never applied: the
agent hands the list to the user, and ADR 0192 stands unchanged.

**Archive all** is the owner's button on the Current section. A preflight
counts what goes and what stays. The bulk route archives only the ids the user
confirmed, and only those still safe. Pinned sub-threads stay open. The result
toast offers Undo, which calls a new owner-only unarchive route that emits
`ThreadUnarchived`.

## Rationale

**Both callers ask the same question, so one function answers it.** A second
spelling of "safe" would drift, and each side's tests would still pass.

**The engine checks approval rather than trusting the prompt.** A tool
description can ask an agent to wait for the user, and nothing enforces it.
A proposal event plus a reply after it is a fact the engine can read. It cannot
tell yes from no, so the user's reply is the gate and the fresh re-check is the
floor.

**Triage widens ADR 0310's reach, and only behind that gate.** ADR 0310 kept
an agent to its own family because nothing showed the user wanted more. An
approved triage is exactly that showing. The plain `archive` action keeps its
ladder.

**Archive all must have no irreversible side effect, so Undo is honest.** The
cascade cancels orphaned question cards, clears external-repo pending changes,
and cancels live event waits. The first two are need facts and the third makes
a thread busy, so the bulk path never reaches any of them. Undo restores the
batch as it was, an unattended trigger run included.

**Delete stays out of agent reach.** ADR 0192 weighed agent delete behind the
owner's instruction and rejected it. A reply-after-proposal gate is the same
shape of authority. It does not change that answer.

## Consequences

- An agent can put away any inbox thread, but only through a proposal the user
  replied to.
- `ThreadTriageProposed` and `ThreadUnarchived` are new persisted thread events.
- Unarchive exists as a route. Undo was its only caller until ADR 0378 added
  Move to Current, which takes the sub-threads too.
- A thread waiting on the user is never bulk archived. A monitoring trigger
  that asks every run still leaves one open thread per unanswered run. Triage
  groups those runs per trigger and proposes dismissing older questions.

## Alternatives considered

- **A bare "archive everything" button.** Rejected: it would cancel question
  cards and clear external changes in bulk, with no way back.
- **Prompt-only approval.** Rejected: nothing enforces it.
- **An engine-rendered approval card inside `apply_triage`.** Strongest, and
  costly: a tool handler would have to park the agentic loop on its own card.
  The reply gate plus the re-check covers the risk at a fraction of the cost.
- **Agent delete behind the triage gate.** Rejected per ADR 0192.
- **Undo by pinning and unpinning.** It works on today's events and lies in
  the event log: the user never pinned anything.
