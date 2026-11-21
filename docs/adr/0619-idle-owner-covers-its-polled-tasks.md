# 0619: An idle coding agent never leaves its own running work unwatched

- **Status**: Accepted
- **Date**: 2026-11-02
- **Amends**: [0257](0257-coding-agents-wait-through-engine-owned-background-tasks.md).
  Its event wait now also covers the owner's polled tasks once its turn ends.
- **Amends**: [0470](0470-keep-the-prompt-cache-warm.md). A polled task is the
  owner's job only while its turn runs.

## Context

ADR 0257 delivers a background task's result through an event wait the engine
arms over the task. ADR 0470 added the *polled background task*: the owner
keeps its turn and calls `wait` until the task finishes. So the engine armed
no wait for a polled task.

That holds only while the owner's turn runs. A coding agent sometimes ends its
turn while its polled tasks still run. Nobody then polls, no wait covers the
tasks, and their completions reach no one. In the observed case `/harden`
started its gates and its other-model review as polled tasks, and the turn
ended at once. Both tasks finished within three minutes. The thread stayed
idle until the user tapped Harden.

The coding-agent turn end also never armed a wait at all. Only the chat turn
tail did.

## Decision

- **An idle owner never leaves its own running work unwatched.**
- **A polled task is the owner's job only while its turn runs.** At a clean
  turn end the engine covers it like a watched task. One event wait covers
  every uncovered task.
- **The first completion re-opens the thread.** The re-opened turn ends again,
  and the turn end re-arms over what still runs. So the engine resumes per
  completion, not once all tasks are done.
- **Only a clean turn end arms.** The turn must end as generated output, not
  as a merge resolution and not as a cache keep-alive. A user Stop, a
  superseding follow-up, a failure or an abort arms nothing.
- **The arm comes before the idle's other decisions:** the live-wait hold, the
  turn-end gate and the idle emit.
- **A result that finished unread is delivered too.** A polled task can finish
  mid-turn while the agent never calls `wait` again. Nothing runs at the clean
  turn end, and the turn-gap note skips a completion that landed before the
  idle. So the turn-end wait also covers each *unread result* until a wait
  delivers it.

`plan_wait` takes the owner's turn state, `OwnerTurn`. `Running` (the spawn
route) excludes polled tasks and unread results, and `Ended` (both turn tails)
includes them.

### Tracking an unread result

- The task registry keeps a per-thread list of unread results, outside the
  task entry, because the sweep drops a finished entry after 300 s.
- A polled task joins it at its finish, under the same lock and before its
  completion is emitted. A stopped task never joins (ADR 0369).
- It leaves when its owner reads the finished result through `wait`, `output`
  or `bash_output`, from memory or from the persisted row. It also leaves just
  before one of the owner's waits emits its delivery. A Discard or Archive
  clears the whole thread.
- **It leaves on delivery, never on arming.** One wait delivers one event. A
  wait over two tasks fires on the first, so the second stays unread and the
  next clean turn end arms for it. Two unread results take two re-opens.
- A live engine-armed wait over an unread result covers it, since its
  delivery is already on the way. A model's wait does not: it may have armed
  above the completion and never fire.
- The arming lowers the wait's watermark just below the earliest unread
  completion, so the catch-up scan finds it at once. A completion not yet
  committed needs no lowering: it lands above any committed horizon.

## Rationale

- **The event wait already has every property this needs.** It brings a
  one-shot gate, the task's deadline and the live-wait caps. It shows a
  waiting row with **Stop waiting**, survives a boot and honours ADR 0369's
  own-stop stand-down. It re-enters a coding agent through the same lane as
  any wait, and the cache keep-alive treats it as keep-warm.
- **Per completion suits the work.** In the observed case the gates failed
  first. The agent could start fixing them then, rather than wait for the
  slower review.
- **A Stop must win.** Arming only at a clean end means the engine never
  re-opens a thread the user just stopped.
- **The order carries meaning.** A live wait holds the idle proposal (ADR
  0395), so an unfinished `/harden` reads Waiting, not "hardening missing". A
  child that holds a wait has not finished for its parent (ADR 0254).

## Consequences

- **Guidance still says wait in-turn.** A polled `wait` is cheaper than a
  re-opened turn. The engine prompt and the CLI knowhow add one sentence: if
  the turn ends first, the engine re-opens the thread when the task finishes.
- **A wait armed at a clean idle survives a later Stop**, as a watched task's
  wait always did. The user ends it with **Stop waiting**.
- **A result can arrive twice.** Say a user message opens a turn that polls a
  task an earlier idle armed for. The completion then also lands in that turn
  as a note. A watched task behaves the same.
- **An engine restart forgets unread results.** The list lives in memory. A
  restart already reports a task it lost through the turn-gap note's
  `abandoned` row, and a result that finished before it stays readable with
  `output`.

## Alternatives considered

- **Resume through `ContinuationRequested`.** That is the user's Continue
  button. A completion is not a Continue, and it would bypass the wait's caps,
  waiting row and boot rebuild.
- **Resume once all tasks finish.** It delays the first actionable result
  behind the slowest task.
- **Arm at every turn end, Stop included.** It overrides the user's Stop.
- **Keep the unread flag on the task entry.** The sweep drops the entry 300 s
  after the finish, and the flag with it.
- **Deliver an unread result through the turn-gap note.** The note skips a
  completion that landed before the idle, and the idle re-opens nothing.
- **Fix it in guidance only, by telling agents never to end the turn.** The
  agent already had that rule and broke it. An engine invariant holds for
  every model and backend.
