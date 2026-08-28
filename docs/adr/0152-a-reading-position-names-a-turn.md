# 0152: A reading position names a turn, never a pixel offset into a windowed transcript

- **Status**: Accepted
- **Date**: 2026-08-28

## Context

A thread stopped reopening where the reader left it. Sometimes it opened at the
top of a thread they had been reading the middle of, and sometimes it landed on
the wrong content.

Both come from one cause. The transcript is WINDOWED: ThreadView renders a
trailing slice, and the slice's top edge is session state, re-seeded from the
newest turns on every open (`threadWindow.ts`, ADR 0081). A pixel offset in
`localStorage` measures from an edge that has since moved. It either overshoots
the new slice, which parked the reader at the top, or points at different
content once the thread has grown.

The step-budget window made this routine rather than rare on a step-heavy
thread. The re-seeded slice is far shorter there than the one the reader had
scrolled through.

## Decision

The transcript records **which turn** the reader was parked on, plus that turn's
own offset from the viewport top: `{ eventId, relTop }`, stored as
`anchor:<relTop>:<eventId>`. Where the seed did not take that turn, ThreadView
walks the render window up to it. One budgeted round per commit, and the restore
lands off the growth.

## Rationale

A turn does not move. Its identity survives a re-seeded window, a resize, and
turns appended below it, none of which a pixel offset survives. `relTop` carries
the sub-turn pixels, so the restore is exact rather than "somewhere near".

The id is the `data-event-id` an exchange root already carries, which the
deep-link resolves against too. Sharing it means the two cannot disagree about
which turn is which.

The walk is CHUNKED. Seeding straight to the anchored turn would render every
turn in between in one pass, which is the blocking render ADR 0081 exists to
prevent. Chunking is the third answer that ADR names, beside windowing and
moving work off the main thread. Each round goes through `growRenderWindow`, the
path the scroll-up expansion already uses, so the reader is held still.

The walk is UNCAPPED in depth, because a cap is an approximation. Landing the
reader short of where they were is the failure being reported. It is bounded in
time instead, by `ANCHOR_RESTORE_CEILING_MS`. A transcript that grows for ever
cannot then hold the restore's observers for the life of the thread.

## Consequences

Anchoring is an opt-in (`anchorsToContent`), like `followsLiveEdge`. Only the
transcript is windowed, and only the transcript stamps ids on its children. The
content pane and the thread drawer keep recording plain offsets through the same
hook.

No migration. A stored number still parses and still restores, and the first
scroll replaces it with an anchor. An older build reading an anchor gets `null`
and opens per the seed, the same downgrade `LIVE_EDGE_VALUE` already accepts.

ADR 0064's list of what may move the transcript is unchanged. A restore
returning a reader to the position they left is already on it. Only the
representation of the position changes.

A RESOLVED anchor may reach for the container's maximum, and that is not the
clamp ADR 0064 forbids. The named turn is taller than the part scrolled past, so
it is on screen at the maximum. Refusing would give the top of the window
instead. A bare offset keeps refusing, naming no content, so clamping it would
invent the live edge.

Two things put a resolved anchor past the maximum, and they are told apart by
size. An overshoot of one pixel is rounding: `relTop` and both heights are whole
numbers off a fractional layout, so a bottom-parked reader can measure past the
reported edge. That lands at once. A larger one is content below the anchor
still rendering, or since shrunk. That waits, and takes the maximum only when
the wait is over: `onDeadline`, and the dead-link rescue.

## Alternatives considered

**A bottom-relative offset.** Survives a re-seeded window, because it measures
from the end rather than the start. It breaks the moment a turn is appended,
which is the second half of the report, and a live thread appends constantly.

**Persisting the render window beside the offset.** Rebuilds the old pixel
geometry from both ends. The stored floor is an index into a list that grows,
and the offset still breaks when content ABOVE the reader changes height: a
different pane width, an image that decodes. The anchor subsumes both.

**Seeding the window straight to the anchored turn.** One render of every turn
between the tail and the anchor, which is the blocking render ADR 0081 forbids.
Measured at ~182ms of synchronous markdown for the thread that motivated the
window, against ~38ms for the seeded slice.

**Rendering the window FROM the anchor rather than as a tail.** Breaks
streaming, the standing follow, `hasMoreAbove` and the scroll indicator. All of
them read the window as a contiguous tail ending at the newest turn.

**Restoring to the closest reachable point when the turn is out of window.**
Not a restore. Approximating is the reported failure, so the walk waits instead.
