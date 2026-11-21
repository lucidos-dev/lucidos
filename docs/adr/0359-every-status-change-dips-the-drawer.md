# 0359: Every status filter change dips the drawer

- **Status**: Accepted
- **Date**: 2026-10-04
- **Extends**: 0291

## Context

ADR 0291 made the drawer's Threads/Filters swap dip through the pane
background. The navigation cover keyed on one thing: whether the filter panel
was open.

A status change from the panel closes the panel, so it dipped. "See all
statuses", under a status view's rows and in its empty state, changes only the
status. Its cover key never changed, and the list replaced itself in one frame.
The user reported it from an iPhone. They asked for the same transition on
every status change, made structural so no entry point can skip it.

## Decision

- **One owner.** `setDrawerView` is the only writer of the status filter. The
  signal is module-private and `drawerView` is exported as a `ReadonlySignal`.
- **The transition keys on the value.** The drawer's cover key is
  `drawerSwapKey(panelOpen, view)`: `filters` while the panel is open, else
  `threads:<status>`. Any change to either half is one dip.
- **The leaving list is held as a drawing.** With the panel shut, a status
  change leaves a *leaving-view drawing* over the pane: an inert clone of the
  old list. A CSS animation hides it at the dip's midpoint, where the closing
  panel hides on the panel path.

## Rationale

The panel path works because its leaving view, the panel, stays on screen until
the midpoint while the new list renders hidden under it. A status change with
the panel shut had no leaving layer. The drawing is that layer, so both paths
are the same transition by construction.

The arriving list renders at once, exactly as on the panel path. So the heavy
render lands before the dip starts, and the midpoint swap is pure CSS on
elements that already exist. The drawing is a fresh element per dip, so its
animation starts in the same frame as the fresh cover's.

The clone is taken in `getSnapshotBeforeUpdate`, which runs after render and
before the list's DOM changes. That is the one moment the leaving view exists.
The clone drops `id`, `data-thread-nav` and `data-flip-id`, so no row lookup,
FLIP pass or `aria-activedescendant` can find it.

A drawing still on screen when a new dip starts carries itself over, re-keyed.
So a reversal mid-dip holds what the user sees until the new midpoint, and
never jumps to the list hidden under it.

## Consequences

- Every status change plays the dip, whoever makes it. A caller cannot set the
  value without the setter, and the setter cannot change it without the dip.
- The pane title is unchanged: it names Threads or Filters, so a status change
  leaves it alone.
- The Filter button's glyph already followed the value. It crossfades the same
  way on every path.
- Typing a search query still switches the list with no dip. The key follows
  the status filter, not the search.
- Reduced motion swaps in one frame: the drawing is hidden from its first frame.

## Alternatives considered

**Key the cover on the status alone, with no drawing.** The new list would show
at frame 0 and the cover would rise over it. The user sees the snap, then a dip
over the view they already have.

**The arrival cover for a status change.** The leaving list vanishes in one
frame, then the cover clears. That is the blink 0291 replaced, and it differs
from the panel path.

**Render the new list at the midpoint, from a timer.** The All statuses list is
the heaviest render in the drawer. On an iPhone it can outlast the opaque hold,
so the swap lands under a clearing cover. A timer can also drift from the CSS.

**Mount both views and swap them in CSS.** The status views share one scroller
and its scroll memory. Two of them would share a scroll offset, and every
lookup scoped to the list would find two rows.
