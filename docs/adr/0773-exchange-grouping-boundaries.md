# 0773: Exchange grouping decides boundaries from the event and states every continuation handoff in one table

- **Status**: Accepted
- **Date**: 2026-11-22
- **Related**: [0201](0201-the-clock-is-the-only-order.md), [0206](0206-a-spoken-row-is-an-interval.md), [0230](0230-a-paged-transcript-keeps-what-the-page-holds.md), [0284](0284-pending-card-pins-to-the-bottom.md)

## Context

`exchange-grouping.ts` folds a thread's events into exchanges, the cards a
reader sees. Some events open a new exchange. Others are steps inside one.

Several rows can land in the wrong place: a resumed turn, a note inside
streamed text, a call row, a re-anchored card. Each rule below fixes one such
case.

This record restates the design comments of
`crates/lucidos-app/src/store/thread-events/exchange-grouping.ts`, written
before the comment strip removes them. Nobody checked it line by line against
the code.

## Decision

- The fold keeps all its state in one resumable structure, `GroupFoldState`. It
  can stop after any event and continue with appended events.
  `computeExchanges` memoizes per thread on this.
- `isExchangeStartEvent` takes the event, not its type. `EventWaitCanceled` is
  a boundary only when the cause is a user stop. For every other cause it is a
  step. The park itself is never a boundary.
- `BOUNDARY_CONTINUATION_HANDOFF` is a table with one decision for every
  boundary type. `render-order.test.ts` fails if a type reaches it undecided.
  `EventWaitCanceled` is in the table and not in the start set.
- A turn interrupted by a boundary that resumes under its own `req_id` is
  redirected to the exchange below the card. The table decides this per type.
- Coding-agent events fold by the clock, so their table entries need no
  handoff.
- An effort restart resume under `ContinuationStarted` is a step of the turn it
  interrupted. It draws as a restart row and opens no turn.
- A filed report, stopped child or moved-out child note joins a running turn
  as a step. It opens its own turnless card when no turn is running.
- A spoken boundary is turnless by default and holds no turn. It holds one only
  when a running turn continued into it, which is `Exchange.tookTheTurn`. A
  delegated utterance is a turn.
- A call row is filed in the newest exchange, read off the array and not
  tracked by a pointer. A message still waiting and a turnless boundary are
  stepped over. With no such exchange, a reply opens its own boundary and a
  session mark is dropped.
- A re-anchor moves an exchange as far down as it can without crossing a caller
  utterance or a waiting message. It stops just above the first utterance that
  followed.
- A row backs out to the start of a streamed text run only when text lies on
  both sides. One walk serves both the clock-filed row and the row lifted from
  an extended run.
- `VOICE_ONLY_STEP_TYPES` without the spoken reply is an explicit per-type list
  of steps that draw no row.
- `GroupFoldState.paged` says whether the server holds older events. Only then
  is a step with no boundary behind it opened as a continuation fragment. Its
  `userEvent` is the step itself.
- `ResponseAborted` both ends its originating exchange and opens a boundary
  exchange. That boundary sits last and draws the AbortPanel.

## Rationale

- `ContinuationStarted` was added to `EXCHANGE_START_TYPES` and to nothing
  else. The resumed turn kept writing into the card above the resume. An
  allow-list cannot tell a considered type from a forgotten one. A table can.
- A user stop has no wake, so nothing else in the transcript reports it. As a
  step it lands inside the current turn or rewrites the arming row far above
  the reader. As a boundary it reads like the Stop and Restart turns.
- An effort pick is one turn to the reader. A separate card for the resume read
  as the explanation of the turn above.
- Starting a turn and holding one are different facts. Only holding decides
  whose status a card reports.
- A pointer to the bottom exchange would need clearing everywhere it can
  change. An absorb moves a message, and an abort, a cancel and a user stop
  push a panel and return early. A pointer missed all of them.
- A row filed in a waiting message takes it out of the queue, beyond Stop's
  reach. A row filed in a turnless boundary cannot be read.
- Lifting a row that merely follows a finished run puts a note above the whole
  answer it came after. Two copies of the walk would order the same three
  events by arrival timing.
- Moving to the end regardless prints the caller's words out of order. Refusing
  to move leaves the exchange above a card it was engaged with after. One
  utterance in the way should not cost the move against every other boundary.
- Nothing in an event's shape says whether the renderer draws it, so
  draw-nothing steps are listed per type.
- A paged page that starts mid-turn and a corrupt thread both arrive as steps
  with no boundary. Only `paged` tells them apart.

## Consequences

- Adding a boundary type forces a handoff decision, or `render-order.test.ts`
  fails.
- The gated tool call and its result still re-route by `tool_called_event_id`.
  Only the genuine continuation moves below a permission card. An idle wake is
  unaffected.
- Position is not part of an exchange's own state, so a reorder needs no
  `touched` bump. The render pass recomputes `isLast`, `hasPriorActive`,
  `priorModel` and `priorEffort`, and `chatExchangePropsEqual` compares each.
- `groupIntoExchangesCached` must rebuild when `paged` changes, because the
  flag lives on the fold state.
- A new step type that draws no row must be added to the explicit list by hand.

## Alternatives considered

- Admitting boundary types through an allow-list alone is rejected, because it
  cannot record a type that was considered and declined.
- Treating `EventWaitCanceled` as a step for every cause is rejected for a user
  stop.
- Tracking the bottom exchange with a pointer is rejected, because it missed
  the early-return paths.
- Moving a re-anchored exchange always to the end, or never moving it past any
  boundary, is rejected.
- Deciding by a clever predicate whether an event draws a row is rejected in
  favor of an explicit list.
- Letting a row that follows a finished text run lift to that run's start is
  rejected.
