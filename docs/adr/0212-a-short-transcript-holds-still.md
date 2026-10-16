# 0212: A transcript shorter than its pane holds still: the turns flow from the top

- **Status**: Accepted
- **Date**: 2026-09-17

## Context

A reader watching a live thread reported a big gap between the newest row and
the composer, twice, from a call. The fix shipped that morning read the gap off
its own measurement of a SHORT thread: one shorter than its pane rests its turns
at the top, so the unused viewport under them is a hole above the composer. It
gave the turns their own box, `.thread-feed`, floored it to the room the
scroller leaves it, and rested its content on the bottom.

A bottom-anchored block grows upward. Every new turn made the block taller while
its bottom stayed pinned, so its top travelled up the screen by the new turn's
height. Content the reader had already read moved, from the first turn on, on a
transcript that could not scroll at all. A call is the loudest case, because
every utterance is its own short turn.

That was reported within hours: "Follow live edge should not scroll us down
before a whole page", and then "When there is no scroll, we don't scroll to
bottom.. simple." Asked to confirm the trade, the reporter also corrected the
diagnosis. The gap they reported was on a thread that DOES scroll: "The gap was
there for threads that have a scroll! I wouldn't report it as a bug otherwise."

## Decision

Below one page of content, nothing in the transcript moves. The turns flow from
the top of the feed, and a turn already drawn stays where it was drawn when the
next one arrives. The feed carries no alignment and no floor, at any width.

The space that leaves under the last turn is unused VIEWPORT, and it is
accepted. Nothing is reserved to fill it, and nothing rests the content into it.

## Rationale

**Already-read content is the thing that must not move.** The reader's own
scrolling is the only motion they have asked for, and a transcript with no
scroll has nothing to ask for. Closing the space under the newest turn buys a
tidier bottom edge. It pays in the one currency a reader notices, which is the
line they were reading sliding away.

**The two asks are mutually exclusive, and the reader picked.** Either the
newest turn hugs the composer and everything above it travels, or read content
stays put and there is space below. No layout gives both, because bottom
anchoring IS the travel.

**A short thread is not the reported bug.** The superseded fix found a real
number in a case nobody had complained about, and answered that instead. The
reported gap is on a SCROLLABLE transcript and stands open. Its cause is not
the transcript's bottom padding, which reserves only `--prompt-fade +
--nav-focus-reach` (1.375rem) under the last turn at the live edge.

**The follow never moved this reader.** It writes `scrollTop = liveEdgeTop`,
which is 0 on a transcript that does not overflow, and 0 is where the reader
already sits. The fix belongs in the layout, and `scrollState.ts` is untouched.

**The floor went with the rest, because it only ever served it.** A percentage
floor on the feed is inert with no alignment to distribute it. On mobile it was
also a three-way arithmetic agreement between two measured CSS vars and a
percentage. Wrong by a pixel, every short thread scrolls by a chrome's worth of
nothing. Keeping a mechanism whose only consumer is gone is how the next round
gets one line from re-enabling it.

## Consequences

- `.thread-feed` keeps its box and loses its two declarations. Two things still
  select through it, the last-turn padding rule and the reading position, so the
  element stays.
- The mobile `min-height` override is deleted with the desktop floor. The
  chrome subtraction it performed has nothing left to pay for.
- The space under the last turn on a short thread returns: 173px on desktop,
  376px on mobile, as measured in
  `docs/plans/2026-09-17-the-transcript-rests-on-the-live-edge.md`.
- **The originally reported gap is still open**, on a scrollable transcript
  during a call. It needs its own measurement rather than a third guess.
- `styles/__tests__/a-short-transcript-holds-still.test.ts` replaces the scan
  that pinned the rest. It bans an alignment and a percentage floor on the feed
  and the scroller, and keeps the two older checks: the scroller stays a block
  container, and neither rule reads follow or liveness state.
- Every check in that scan asserts an ABSENCE, so it carries a liveness
  tripwire. Otherwise a reader that finds nothing passes it without reading CSS.
- `e2e/transcript-ends-where-its-content-ends.spec.ts` branches again on whether
  the transcript overflows. The scrollable rounds keep the air assertion, and
  the short rounds assert three things: the turns start at the top of their box,
  the transcript does not scroll, and a drawn turn does not travel across a
  round.
- ADR 0064's "the transcript's height is a function of its content and nothing
  else" holds again without an amendment beside it.

## Alternatives considered

**Keep the rest and stop the follow writing instead.** This is what the second
report sounds like read literally, and it fixes nothing. The follow writes 0 on
a transcript that does not overflow, so the motion was never a scroll.

**Gate the follow's write on `isScrollable` anyway, as belt and braces.** It
would cover a transcript overflowing by a few pixels of chrome. Rejected on two
counts. With the floor gone there is no such overflow. And a 10px gate would
strand a reader 10px short of a live edge they did ask for, with the down
chevron lit over nothing.

**Rest the turns only once the thread has more than one of them.** It answers
the loudest frame of the report and nothing else. The second turn's arrival
still moves the first, which is the same bug one turn later.

**Reserve a screenful under the newest turn so it can reach a landing line.**
Rejected twice already, in ADR 0064 and ADR 0080. It is reserved air, and air
below the last turn lies about how much thread there is.

**Keep the rest and accept the travel, on the grounds that a caller watches the
bottom.** Weighed and rejected by the reader, who reported it within hours of it
shipping. It also fails the wider rule the transcript is built on: the position
belongs to the reader (ADR 0064).
