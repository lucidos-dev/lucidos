# 0261: The transcript fill charges its caps only for rounds that drew something

- **Status**: Accepted
- **Date**: 2026-09-23

## Context

The render window's fill grows the window, or fetches a page, until the
transcript scrolls (`threadWindow.fillAction`). Two caps bound it:
`MAX_FILL_EXPANSIONS` grow rounds and `MAX_FILL_BACKFILLS` pages. ADR 0230
accepted a consequence: a page that draws no height costs a request, the cap
bounds it, and the chevron is the way out at the cap.

A reader on the iOS PWA hit that exit and could not use the transcript. With
steps hidden, a coding-agent turn is mostly rows that draw nothing: one
reported turn ran 352 tool calls between two lines of prose. The row budget
counted those rows, so the seed and every grow round landed inside the silence.
The pages ran out before any prose loaded. The reader saw the turn's header
over an empty body, and nothing scrolled.

## Decision

The row budget counts rows the reader's view draws (`rowsDrawnByClamp`). A fill
round that made progress yet drew nothing is refunded, for grows and pages
alike (`settleFillLedger`). The caps now bound rounds that drew something, and
a failed page stays charged.

## Rationale

A row that draws nothing renders `null`: no markdown and no DOM node. So a round
of such rows costs almost nothing, and charging it against a cap spent the
reader's way in for no saving.

The loop still ends. A refunded grow moved the window's edge up, and a refunded
page moved the history floor older. A thread has only so much of either.

Progress for a page is the history floor, the oldest loaded sequence, and never
the event count. A live turn streams events in at the newest end. A count would
read a failed page as progress there, and retry a broken endpoint on every
resize.

`ROW_CEILING` bounds how many rows one round uncovers, drawn or not. The edge
is not re-seeded when the reader shows steps, so rows hidden by the view today
all draw at that moment. The ceiling bounds that commit.

## Consequences

- ADR 0230's "the cap bounds a page that draws no height" no longer holds. Such
  a page is fetched until prose loads or the thread reaches its first event.
  The chevron stays available, but it is no longer the planned escape.
- A failing history endpoint is still asked at most `MAX_FILL_BACKFILLS` times.
- Crossing a long silent run takes several rounds, each uncharged.

## Alternatives considered

- **Raise the caps.** Any fixed number is beaten by a longer silent run, and the
  reported thread already beat the page cap.
- **Render the whole turn once the fill stalls.** The window exists for render
  cost (ADR 0081), and a live coding-agent turn runs to thousands of rows.
- **Re-seed the edge when the reader toggles steps.** It needs a second rule for
  which hidden rows may leave, and `ROW_CEILING` bounds the same cost without
  touching the edge.
