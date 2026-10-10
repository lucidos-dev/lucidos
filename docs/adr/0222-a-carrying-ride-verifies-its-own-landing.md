# 0222: A carrying ride verifies its own landing, rather than trusting the write

- **Status**: Accepted
- **Date**: 2026-09-19

## Context

The *standing follow* writes the live edge from one place, `honourGrowth`'s
carrying arm, on one event: the transcript's ResizeObserver. Nothing read the
result back. `holdPosition` stamps from the same measurement the write used. So
a write that came to rest SHORT recorded itself as a landing on the edge.

Nothing corrected that. The next growth round would, and a coding-agent turn
can spend sixteen seconds inside one tool call. Across that gap an armed reader
sat off the live edge with the toggle lit and the down chevron lit beside it.

Reported three times from the iOS PWA. The first two rounds moved layout and
were both reverted the same day (ADR 0212). The third round carried a
screenshot and the thread's own event history, which is what made the sixteen
seconds measurable. Full reasoning:
`docs/plans/2026-09-19-a-carrying-ride-verifies-its-own-landing.md`.

A second hole sat beside it. `keepTheLiveEdge` required a reading that the
reader WAS on the edge before the event, from a measurement or from the app's
own placement. The round that declines a correction clears both together: the
disarm's `forgetHeldLiveEdge` takes the claim, and `recordAnchor` re-takes the
measurement off the edge. The correction could then never fire again. One arm
of that was closed before, for the placement case, and the other three
stand-downs still reached a carrying rider.

## Decision

A ride that is CARRYING verifies its own landing. The growth round schedules one
frame, which re-measures and writes the edge again where the write fell short.

And a carrying ride needs no reading of where the reader was. `keepTheLiveEdge`
takes `followIsCarrying()` as a third source beside its two position readings,
matching what `honourGrowth`'s carrying arm already does.

> **Amended:** ADR 0064's follow is one state again, so every armed ride is a
> carrying one. `keepTheLiveEdge` reads the armed flag alone, and both position
> readings and `followIsCarrying` are gone.

## Rationale

The ride is a standing request, not a reaction. Serving it on events alone makes
every missed write permanent while the thread is quiet. The thread is quietest
exactly when the agent is doing the slow work the reader armed the ride to
watch.

Verification is cheap and bounded. One frame per growth round, reading two
numbers, writing at most once, scheduling nothing after itself.

**The stamp is what makes it safe.** `isWhereWeHeldIt` says the container is
still exactly where the ride left it a frame ago. Anything that moved it since
fails that term, so the check can no more fight the reader than it can fight a
navigation. Neither needs a term of its own. That is the same reading the
disarm takes for the same question, so the two cannot drift.

The position term in `keepTheLiveEdge` was never deciding anything for a
carrying rider. The growth branch takes such a reader to the edge on the next
row whatever the term says. All the term did was strand them when an earlier
round happened to record them off the edge.

## Consequences

- An armed reader on a live thread comes to rest on the live edge, and the
  chevron settles with them.
- The correction now reaches a carrying rider the round after a declined one,
  instead of never.
- The ride posts one `[Client/follow] short` breadcrumb per ride when it
  corrects a short landing. It is a diagnostic with a removal condition, in
  `docs/temporary-measures.md`. The TRIGGER for a short landing is still not
  measured on the device it happens on.
- A reader who is armed on a QUIET thread is unaffected. They keep both position
  readings, since nothing is arriving to carry them toward.
- The layout is untouched. Both earlier rounds moved it and both were reverted.

## Alternatives considered

- **Hunt the trigger first, and ship only the breadcrumb.** Offered to the
  reporter as the second option and declined. It carries no regression risk and
  fixes nothing, and the report is on its third round.
- **A poll, or a rAF loop while the ride is armed.** Rejected. It would run for
  the life of every armed thread, to catch a case that is rare. This codebase
  bans a poll where an event will do, and one frame hung off a write that
  already happened is not a poll.
- **Re-measure synchronously, inside the same round.** Rejected. The
  ResizeObserver callback already runs after layout, so a second read there
  returns the same number. The whole failure is that the extent settles LATER.
- **Drop `keepTheLiveEdge`'s position term outright.** Rejected. It is what
  stops the ride hauling an armed reader out of history on a quiet thread, which
  is ADR 0064's central promise.
- **Drop the caller's `isPlacementScroll` guard too.** Rejected. A reveal, a
  chevron and a deep link put the reader somewhere on purpose, and each already
  retires the ride itself where it should.
- **An `isScrollable` gate on the follow's write.** Rejected in the previous
  round and again here. It would strand a reader short of an edge they asked
  for, with the chevron lit over nothing.
