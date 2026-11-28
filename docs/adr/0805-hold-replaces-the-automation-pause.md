# 0805: Hold replaces the automation pause: a thread and its sub-threads run nothing by themselves until Resume, with no cards

- **Status**: Accepted
- **Date**: 2026-11-23
- **Amends**: ADR 0664 (the automation pause), ADR 0696 (the Stop hold gains Hold beside it), ADR 0328 (the Set aside verb)

## Context

The automation pause of ADR 0664 talked only through cards. Each automatic
step that came due raised a permission request with the step once,
**Unpause** and **Keep paused**. Nothing else showed that a thread was
paused, and nothing else offered Unpause. One recorded pause asked four times
in three hours.

The event log showed what owners wanted. Every recorded pause was one of three
things: park work for later, free the machine, or mark work that moved
elsewhere. Each means "this work does nothing until I come back". None used
the step cards, and none needed Unpause as a separate act.

## Decision

**Hold** is a state of a thread of any kind, placed on a root and reaching
every unarchived sub-thread. While it stands, Lucidos starts no turn there by
itself. **Resume** on the root lifts it.

| Verb | Reach | Way back |
|---|---|---|
| **Stop** | This thread's turn | The owner's next message, or Continue |
| **Hold** | The thread and its sub-threads | **Resume**, or Archive |

- **Events.** `ThreadHoldPlaced { reason, hold_id, root_thread_id }` on every
  thread the Hold reached, and `ThreadHoldLifted { hold_id, cause }` with
  cause `resume` or `archived`. A stored `AutomationPaused` folds as a hold
  rooted at its own thread, with its event id as the hold id.
- **One reader.** `core::store::thread_hold::may_start_turn` answers "may the
  engine start a turn here?" for a Stop, an open card and a Hold alike. Boot
  resumes, report wakes, event-wait deliveries, turn-end nudges, ceiling
  handoffs and model-swap re-entries all read it.
- **Hold stops running work.** It stops every running turn in the tree with
  `CancelCause::Hold`, which leaves no Stop hold behind. The holder's own turn
  ends by itself, so its reply can say what it checkpointed.
- **Hold sets pending changes aside**, tagged with the hold id. Resume brings
  back exactly those.
- **Hold raises no card.** A message from the owner still runs a turn, and
  the thread stays on hold.
- **Resume runs on the root only.** A reached sub-thread refuses it and names
  the root. A sub-thread already held or stopped before the Hold stays so.
- **Who may hold.** The owner, an agent for its own thread or one below it,
  and Home for any thread in its workspace.
- **Who may resume.** The owner, and the thread's own agent only inside a turn
  the owner's own device message started there. A parent, Home and a trigger
  may not.
- **Archive lifts.** The owner's archive of a held thread lifts its hold.
- **Routes and CLI.** `POST` and `DELETE /api/v1/threads/:id/hold`, answering
  `{held, changed}`. `lucidos threads hold --reason` and `lucidos threads
  resume`. The old `/automation-pause` route and `pause-automation` command
  stay as one-release aliases (`docs/temporary-measures.md`).
- **A held parent files its child reports silently.** A coding-agent parent
  reads them after Resume. A chat parent reads them with the owner's next
  message, since Resume asks only a coding agent to read reports.
- **Upgrade.** At boot, every open or queued step card a pause raised retires
  as `Superseded`, which delivers the messages that waited behind it.

## Rationale

Hold names the one thing owners did with a pause: park the work. A state the
owner can see and end in one place replaces a stream of cards that each asked
the same question. Sharing the Stop hold's reader means one definition of
"held", so no wake path can miss a Hold that another path honours.

Reaching the tree is what "free the machine" needs: an orchestrator's
sub-threads keep running otherwise. Setting changes aside under the hold id
keeps Review honest while the work waits, and lets Resume undo exactly what
the Hold did.

## Consequences

- The step cards, `ask_before_step` and the Unpause choice are gone. Stored
  cards and the stored `unpause` choice still read, as `Superseded` and
  Ignored.
- A Hold is durable: no message, Continue or report ends it. Only Resume or
  the owner's archive does.
- Codex and Claude Code sessions and chat threads all hold, through the same
  reader.

## Alternatives considered

- **Keep the cards, fix their liveness.** A pause would keep asking, and every
  card bug class would stay reachable.
- **Hold as the Stop hold, ended by any message.** The owner sees Stop as
  transient and Hold as durable. A question to a held orchestrator would
  restart its whole tree.
- **Park as the word.** "Parked" already means "waiting on one thing", and
  users see it in "parked on a question".
- **A run-once button per waiting step.** More states than any recorded use
  needed. A message already runs one turn.
