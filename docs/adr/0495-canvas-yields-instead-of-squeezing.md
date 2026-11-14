# 0495: A Canvas that cannot fit its floor is drawn collapsed, from a derived drawn split ratio; the stored ratio is never rewritten

- **Status**: Accepted
- **Date**: 2026-10-16
- **Amends**: [0056](0056-pane-dividers-clamp-instead-of-snapping.md)

## Context

A user reported header icons drawn on top of each other "when canvas is
hidden": the Home rail toggle sat on Search everywhere in the window corner.

The Canvas pane was not collapsed. It was squeezed to a sliver, 1 to 43px wide.
Three things shrink the split: a narrower window, an open thread drawer, and a
shown Home rail. The pane floors are derived from the root font size, so at
125% ui-scale they stop fitting a 1280px window with the drawer open. ADR 0056
answered that case with `clampToRange`: the leading pane keeps its floor and
the trailing one takes what is left. What was left could be one pixel.

`data-content-collapsed` stayed false, so every Canvas header control stayed
docked over a pane too narrow to hold them. The rail toggle is fixed to the
window corner, and with no Canvas row under it, it landed on the Conversation
row's Search. A Canvas a few boxes wide also overlapped its own row.

The user's rule: header icons never overlap.

## Decision

The split draws a **drawn split ratio**, derived from the stored ratio, the
split's width and the two pane floors (`computeDrawnSplitRatio`,
`components/layout/drawnSplit.ts`). With room for both panes it is the stored
ratio held to both floors. With room for one, an open Canvas **yields**: it is
drawn collapsed. The stored ratio is never rewritten for lack of room.

## Rationale

A pane narrower than its floor has a header that cannot fit its controls. The
floor exists to say so. A squeeze below it leaves a sliver nobody can use, and
its controls land on its neighbour's.

Drawing collapsed reuses the state the header already knows how to lay out.
Every collapse rule (the regions that fade, the toggle that docks elsewhere)
keys on `data-content-collapsed`, so a yielded Canvas inherits all of them.

The stored ratio stays the user's choice. A drawer closed or a window widened
brings the Canvas back where the user left it. Rewriting the stored ratio would
lose that, and it is how the earlier one-shot migration worked: it persisted a
clamp at three moments, and the screen was wrong at every moment in between.

## Consequences

- **ADR 0056's "the trailing one takes what is left" no longer reaches the
  screen.** `clampToRange` still answers an empty range for a drag in flight,
  but the drawn ratio never draws that remainder at rest.
- **A drag holds the collapse state** (`beginPaneResize` pins it, `endPaneResize`
  releases it). A drawer or rail drag changes the split's width, and a yield
  flip under the pointer would bring back the icon dance ADR 0056 removed. The
  Canvas yields or returns at release. A split divider drag in a split too
  narrow for both floors moves nothing, since collapsed is its only position.
- **Showing a yielded Canvas swaps the panes** (`contentShownRatio`). When only
  one pane fits, every way to show the Canvas sets the stored ratio to 0: its
  toggle, a content navigation, both double-clicks, and `⌘⇧↵` from the Canvas.
  Restoring the default split there would yield again and do nothing visible.
  The one exception is an agent navigation while the reader types: the swap
  would hide the field being edited, so the Canvas stays yielded.
- **`⌘⇧↵` from the Conversation is a no-op while the Canvas has yielded.** The
  Conversation already fills the split, and there is no split to restore.
- **The Home rail toggle never shrinks away.** With the Canvas hidden, it docks
  in the Conversation row's trailing slot. That row keeps room for it the way
  the Canvas row does, whether a toggle hid the Canvas or a yield did.
- **`migratedSplitRatio` and its three effects are deleted.** The drawn ratio
  holds the floors at every moment, so a migration has nothing left to fix.
- **Screen readers of the ratio read the drawn one; writers write the stored
  one.** A reader that compares against the stored ratio describes a split
  that is not on screen.
- **The Conversation can still be squeezed below its own floor**, by the
  drawer and the rail together on a small window at a large scale. The same
  rule would make the drawer or the rail yield, and each has a toggle that
  would then lie. That is a separate decision, settled by
  [0498](0498-the-conversation-never-squeezes.md).

## Alternatives considered

**Keep the squeeze and hide the docked controls under a width threshold.** It
treats the overlap and leaves a sliver of Canvas no one can use. It also needs a
second threshold beside the floor, which already names the width that fits.

**Rewrite the stored ratio on every resize** (a migration that runs always).
The screen would be right. But the first open drawer would lose the user's
ratio, and a drag would have to fight the writes.

**Yield the Conversation instead.** The Conversation is the primary work area
and the one Lucidos focuses on a fresh load. The Canvas holds what the user
opened from it, so it is the one that steps aside.
