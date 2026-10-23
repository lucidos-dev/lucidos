# 0431: The thread drawer windows its rows behind gaps of measured height, never estimates above the viewport

- **Status**: Accepted
- **Date**: 2026-10-11

## Context

The iOS PWA lagged in step with the page's DOM size. The phone mounts every
pane at once, so the thread drawer's rows cost every frame, even while a
thread is open. With about 400 threads in Current the page held 5,200 to 9,400
nodes and stalled up to 321 ms per 30 s window. Every Folders section rendered
a full row per loaded thread; "Archive is paged" meant network paging only.

The user set two hard requirements. Nothing may jump: not while scrolling, not
when rows arrive or leave, not when a section rolls, not when the window moves.
And navigating to a row (a tap, keyboard browsing, Show in Folders) must land
without a blank frame.

Plan: `docs/plans/2026-10-11-drawer-row-window.md`.

## Decision

Each Folders section renders only the rows near the viewport, plus pinned rows,
and one gap per run of the rest (`components/drawer/rowWindow.ts`,
`WindowedRows.tsx`). A gap's height is the sum of its rows' measured heights.
Above the viewport it is never an estimate: an unknown or stale row there is
rendered instead, and folded into a gap once measured.

## Rationale

**Exact gaps make the window invisible.** If every gap equals the rows it stands
for, moving the window changes no row's position, so there is nothing to hide.
The other way, estimating and then correcting, has to write `scrollTop` when the
real height arrives. On iOS that write stops a fling mid-flight, which reads as a
snap. Exact gaps need no correction at all.

**Staleness is detected, not enumerated.** A height is valid only under the
row's *layout key* and the current environment:

- The layout key comes from the same function the row renders its box from.
  A table classifies every row prop as layout, paint or handler, and TypeScript
  refuses a prop it does not list.
- The environment changes when the list's width changes, or when a mounted row
  resizes under an unchanged key (a font load, UI scale). Either drops every
  height.

**Below the viewport an estimate is harmless.** Correcting a row below the
viewport moves nothing on screen, so unseen rows there wait to be measured
until they enter the window.

**Navigation goes through the window.** The row a keyboard step or Show in
Folders lands on mounts first, with the viewport around it. The scroll happens
in that commit's layout pass. A far reveal approaches from one viewport out: a
smooth glide over hundreds of rows would outrun any overscan.

## Consequences

- The DOM holds about five viewports of rows per section, however long it is.
- The windowed list's geometry equals the full list's. So arrivals, departures
  and FLIP motion behave exactly as before, including where they move the list.
- A deep reload restore renders every row above the saved place once, to
  measure it, then folds them. A width change does the same for the rows above
  the window, and the window does not shrink until the width holds still.
- The FLIP hook diffs logical row ids, so a row entering or leaving the window
  is not a change it animates. It learned one thing: a gap carries the last row
  it stands for (`FLIP_GAP_END_ATTR`). So a section's roll measures the block
  down to its true bottom, and the rows below move in lockstep.
- A draft row is never cached: its title is its text, which changes with every
  keystroke. It renders wherever it sits above the viewport.
- The desktop drawer's body keeps its open width while the drawer slides open
  or shut, and the drawer clips it. Rows used to rewrap on every frame of the
  slide, so a saved offset pointed at different rows until the slide ended.
- The Ongoing grouping and search results stay unwindowed. They hold only what
  needs attention or matched a query.

## Alternatives considered

- **`content-visibility: auto` on rows.** It skips layout and paint but keeps
  every node in the DOM, and the node count is what lagged. It also changes
  `scrollHeight` as rows render, which scroll memory relies on.
- **An off-the-shelf virtualizer (TanStack Virtual and similar).** These place
  estimated rows above the viewport and correct `scrollTop` when the real height
  arrives, the iOS snap above. They also assume one flat list, where the drawer
  has sections, FLIP masks and pinned rows.
- **Fixed row heights.** Titles wrap, and the chips and family line come and go.
  Fixing the height would change the design to suit the mechanism.
- **Holding the visible rows still when a row arrives above them.** That is new
  behaviour, not windowing. It needs `scrollTop` writes, and it fights the FLIP
  sibling slide.
