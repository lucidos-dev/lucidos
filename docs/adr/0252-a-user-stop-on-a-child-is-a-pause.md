# 0252: A user Stop on a child thread is a pause: the parent is told, not woken, and only a new turn, Archive or Discard settles it

- **Status**: Accepted
- **Date**: 2026-09-23

## Context

A parent coding-agent thread spawned a *child thread* to work a ticket. The
child asked a question. The user pressed Cancel on the card, typed a new
direction, and the child kept working. The parent was woken at the Cancel with
`[CHILD THREAD COMPLETED] … canceled (user stop)` and no pending changes. It
read the child as dead and offered to roll the ticket back or respawn the
child. Both were wrong, because the child was alive.

Cancel on a question card is a Stop. It calls the same route and emits the
same `ResponseCanceled { cause: user_stop }`. The fan-in treated every such
cancel as a completion, and `SettleTerminal::CanceledQuestion` said so on
purpose: "a parent waiting on a child the user just cancelled has to hear about
it."

The premise was wrong. A Stop never ends a thread. The child stays resumable,
and one message starts it again. Nothing in the card said so.

## Decision

A user Stop on a child makes it a *stopped child*. The parent gets a
`ChildThreadStopped` event and no turn. The parent is still owed a result, and
one of three things settles it:

- The user continues the child. The parent hears when that turn ends, as it
  already did.
- The user archives the child, or discards its pending change. The parent gets
  `ChildThreadCompleted { status: canceled }` and is woken.
- Nothing happens. The child counts toward attention and says the parent is
  waiting, until the user does one of the two above.

Only a person's Stop qualifies: `CancelCause::UserStop` with a human actor, or
none. An agent cancelling its own child records the same cause, and keeps
reporting a canceled card as before, as does a `UserAction` cancel. Deleting
the child settles it like an archive.

## Rationale

The user is in the child, steering it. The parent has nothing useful to do, and
a woken parent acts. So the wake is the harm, not the text on the card.

Rewording the card was the cheaper fix, and it was rejected. A woken agent
spends a turn and reaches for an action, whatever the card says.

Silence was rejected too. A user who stops a child and walks away would leave
the parent unaware forever, with the ticket still in progress. The attention
count closes that gap without a timer. A timer would need to survive restarts
and would still guess at the user's intent.

The two acts that really mean "done" are Archive and Discard. They told the
parent nothing before. Now they carry the only `canceled` card.

## Consequences

- `parent_callback_pending` stays set through a stop. `is_stopped_child` on
  `thread_summaries` records the state, so the idle that follows a Stop sends
  no card and the settle acts know what they owe.
- A stopped child needs attention but blocks nothing. Archiving its parent
  stays possible, and a cascade from an ancestor sends no card.
- `ChildThreadStopped` is metadata. It starts no turn, and it must not hide an
  unprocessed `ChildThreadCompleted` from the boot re-fire sweep (ADR 0011).
- A waiter on the child's `CodingAgentIdled` still fires on a Stop, because the
  agent really idled. "The child is done" is `ChildThreadCompleted`.
- Top-level threads are unchanged. A Stop on a *top-thread* has no parent to
  tell.

## Alternatives considered

- **Cancel dismisses the card and the turn runs on.** Rejected. The agent
  would act on a `(canceled)` answer while the user types a new direction.
- **Wake the parent with a "paused" status.** Rejected for the same reason as
  rewording: any wake invites the parent to act.
- **Treat only a question Cancel as a pause.** Rejected. A Stop button press
  has the same shape and leaves the child just as alive.
- **Stop emitting `CodingAgentIdled` after a Stop.** Rejected. It would change
  what "idle" means for every trigger and app that reads it.
- **Call the state "paused".** Rejected. *Paused* already means a turn the
  engine resumes by itself, with no attention.
