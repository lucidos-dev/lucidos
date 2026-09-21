# 0232: A render-all lasts one visit, and a gesture at the top asks again

- **Status**: Accepted
- **Date**: 2026-09-20

## Context

ADR 0230 made a paged transcript reachable. Two reports followed, both about
how it feels rather than whether it works.

**Opening one thread was slow every time.** A 13,683-event thread, reached once
from a notification link. A deep link renders the thread whole and persists
that, so the reader is not snapped back to the tail while reading the old
event. Nothing ever narrowed the edge again, and `renderFloorByThread` is
module-scoped, so one tap bought every later open a full render:

| Opening it | Events | Turns | Rows drawn |
|---|---|---|---|
| windowed | 400 | 12 | 37 |
| after one deep link | 13,705 | 226 | 4,069 |

The fold is not the cost, at 10ms for the whole thread. The 4,069 rows of
markdown are, and they are the blocking render ADR 0081 forbids.

**Scrolling up stalled.** The page round shares the grow's trigger. By the time
the reader is within `WINDOW_EXPAND_MARGIN_PX` of the top there is nothing left
to render, so the fetch runs with them stopped.

## Decision

A render-all is a claim one VISIT's navigation makes. A fresh open drops it
back to the ordinary seed, and the *reading position* walks the window to where
the reader was. A partial edge is kept.

The second report is fixed from the other end. The transcript listens for a
`wheel` or `touchmove` while the reader is pinned at the top, and runs the same
decision its scroll handler does. Prefetching ahead of the reader was built and
withdrawn.

## Rationale

The persist was right about its own visit and wrong about the next one. What
the reader needs on re-open is their place. The reading position already names
it as a turn. The walk to it is chunked a round per frame (ADR 0152), which a
full render is not.

Keeping a partial edge is what stops the re-seed becoming a regression. The
reader grew that edge by scrolling. Re-seeding would make them walk back up on
every return, which is what module-scoping it prevented in the first place.

## Consequences

- **A visit ends at the pane teardown, not at the mount's.** A switch, a New
  chat, and the layout swapping at the mobile breakpoint are all one teardown,
  so the mark is module state cleared there. A ref died with the mount, which
  the breakpoint swap performs mid-read.
- **The mark is written past the settle guard.** Written before it, a commit
  whose events had not landed spends the visit. The commit that evaluates then
  reads itself as a later one.
- **The window's own scroll correction goes through `markAnchorScroll`.** It
  was the one app write that did not, which was survivable while the expand
  margin was under one screen and is not at 1600px.
- A reader who deep-links to an old turn, leaves, and returns pays the anchor
  walk to reach it again. Chunked per frame, so the pane stays responsive.
- **A gesture is the reader by definition, so no navigation guard applies to
  it.** The scroll handler needs one, because our own writes fire scroll
  events. Nothing in the app dispatches a wheel.
- **One page per flurry.** `requestBackfill`'s own guard holds, and a wheel at
  the top fires many events per gesture.
- **Only an UPWARD gesture asks.** The event fires before the browser moves
  the container, so a reader leaving the top downward still measures as pinned
  there. Acting on that renders turns they are scrolling away from, then jumps
  them by the height it added. A wheel carries its own delta. Touch does not,
  so direction is the travel between two moves, forgotten when the finger
  lifts.

## Alternatives considered

**Prefetch the page behind the window, a round before the reader needs it.**
Built, reviewed, and withdrawn. It is the right idea, and it does not fit the
machinery underneath it.

Taking it out exposed what it had been hiding, which is the defect this ADR
actually fixes. The scroll handler was the only caller of the backfill, and a
container at `scrollTop` 0 fires no scroll event. So a reader at the top could
not ask again. Walking a six-page thread landed ONE page and then froze: scroll
height reached 5,739 at wheel step 10 and had not moved by step 140. The way
out was to scroll back down and up again.

Listening for the gesture is safe where the prefetch was not, and for one
reason: the page lands with the reader parked at the loaded floor. That is the
state the anchored landing was written for.

Every earlier backfill happened with the reader parked at the loaded floor,
waiting. The landing was built on that: it restores the captured edge and grows
the window.

A prefetch fires while the reader still has runway. So it lands with them
somewhere else, and possibly mid-grow. Three reviewers plus Codex found eight
distinct defects across two rounds. It grew the window under them. It rolled
the edge back, evicting turns already on screen. It could chain-load a whole
thread when a page merged into a *continuation fragment*.

Each fix exposed the next, and the root is that `WindowEdge.exchange` is an
INDEX. Any prepend has to translate it, and translating correctly while the
reader is also moving is a different contract. Making prefetch safe means
giving the edge an identity rather than an index, which is its own change with
its own plan.

What stands instead is the gesture above. The reader still waits a round trip
at the floor, once per page, which is the part of the report this does not
answer. What no longer happens is the freeze.

**Fold a prepended page on its own instead of re-folding everything.**
`prependEventRows` replaces the events Map, so every page misses the memo and
re-folds all of it. Measured and rejected: 1.0 to 1.8ms at up to 3,600 events
held. The fold is order-dependent, so joining two folds means proving nothing
in the cached half routes by id into the new page. A divergence there is a
transcript that reads wrongly, with no failure signal.

**Virtualize the transcript, or give turns `content-visibility: auto`.** Both
are real levers on the render cost, and both are a larger change than either
report needs. Not ruled out.

**Drop the persist entirely, so a deep link never renders all.** Rejected: that
is the snap-back to the tail the persist was added to stop, and it happens
while the reader is mid-read.
