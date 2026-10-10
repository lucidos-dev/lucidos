# 0056: Pane dividers clamp at the pane minimums; the deferred snap is retired

- **Status**: Accepted
- **Date**: 2026-08-09

## Context

Desktop pane resizing was a **free drag plus a deferred snap**. A divider landed
wherever the pointer dropped it; roughly 400ms after release, a pane below its
minimum animated up to that minimum, or disappeared entirely when it was below
half of it. Both dividers behaved this way: the thread drawer's and the split one
between the Conversation and Canvas panes.

The arrangement existed for one specific reason, recorded in
`.claude/rules/frontend.md` § Pane Resize as "Never clamp or collapse mid-drag".
The collapse-state attributes (`data-thread-collapsed`, `data-content-collapsed`,
`data-thread-drawer-open`) decide which host renders which header icon group, so
flipping one mid-drag swapped icons between headers while the pointer wiggled
across a pane edge. Deferring every correction to release made that impossible
*during* the gesture, and the drag kept the ratio 1px off 0 and 1 to be sure.

What it cost was paid on every resize. The divider moved after the user let go,
which reads as the app disagreeing with the drop. Worse, the only way to find out
where a minimum was was to violate it: nothing resisted, so the wall was
invisible until the layout corrected itself past it.

## Decision

A divider drag is **clamped** to the legal range as it moves, and corrects nothing
on release. What the user drops is what persists. **A drag can no longer collapse
a pane**; collapse keeps every other entry point it had (the drawer toggle and
`⌘⇧1`, the pane toggles, `⌘⇧↵`, the divider double-click, the header
double-click). The deferred-snap machinery is deleted rather than bypassed.

## Rationale

The ban on mid-drag clamping was really a ban on mid-drag *collapse flips*, and
this change removes that failure mode more completely than deferring it did.
Those attributes flip at a ratio of exactly 0 or 1, and the drawer's flips only
from its own toggle. A clamped drag cannot reach 0 or 1, because both pane
minimums sit well inside them. So the flip is **unreachable** during a drag,
where the snap merely **postponed** it. The 1px floor the drag carried to
approximate this is retired as redundant.

That is the whole argument, and it is worth stating plainly because the rule read
as forbidding exactly what was implemented: a clamp is safe here precisely
because it is paired with never collapsing.

Two smaller things follow. The keyboard resize shortcuts already clamped
immediately and never collapsed, on the reasoning that "a discrete keystroke has
no mid-gesture state to defer around", so the two input paths disagreed about
where the wall was, and now share one. And a minimum you can feel is a minimum
you can discover; one that only asserts itself after the fact is a correction,
not a constraint.

## Consequences

- **Drag-to-collapse is gone**, for the drawer and for both split panes. This is
  a real affordance removal, taken deliberately. Every other route to a collapsed
  pane is untouched, and the divider's own double-click still does it.
- **A container too narrow to hold every minimum now needs an answer.** The snap
  never faced this, because a free drop was legal by definition and the snap
  could pick a side afterwards. `clampToRange` decides it: the leading pane keeps
  its minimum and the trailing one takes what is left. Not a corner case, since
  the three pane floors are derived from the root font size (see
  `store/paneMinimums.ts`) and stop summing under a 1280px screen somewhere past
  150% ui-scale. (ADR 0058 raised the drawer's floor on the web client, which
  moved that crossing down to 150% exactly.)
- **`computeSnapRatio`, `computeDrawerSnap`, `cancelPendingSnap`, the pending-snap
  timer and `SNAP_DELAY_MS` are deleted.** `beginPaneResize` / `endPaneResize`
  survive as the `data-pane-resizing` attribute pair, which still turns the header
  geometry transitions off so the header tracks the panes 1:1 during a drag.
- **`.snap-animate` becomes `.pane-animate`.** With no snap, the class described a
  caller that no longer existed; it animates an explicit ratio change (a toggle, a
  maximize, a keyboard step, a layout reset) and now says so.
- **Dragging a collapsed divider re-expands the pane to its minimum**, which falls
  out of the clamp and matches what a keyboard step into a collapsed pane already
  did.

## Alternatives considered

**Keep the deferred snap and only tighten the minimums.** The original reading of
the report. Rejected once the second half of the instruction arrived ("we do this
instead of snapping collapse for content pane as well"), and it would not have
addressed the actual complaint: the wall was invisible until crossed.

**Clamp, but keep a drag-to-collapse gesture at the far end of the travel** (drag
well past the wall and the pane closes). Tempting, since it preserves the
affordance. Rejected because it reintroduces exactly the mid-drag collapse flip
this ADR relies on being unreachable, and because the user asked for the collapse
on drag to go rather than to move.

**Clamp on release only, without collapsing** (a snap that corrects up but never
hides). A smaller change, and it keeps the machinery. Rejected for the reason the
whole change exists: the divider would still move after the drop, and the
minimum would still be discoverable only by violating it.

**Leave the two split-pane minimums as px constants** while clamping. Rejected on
review: with the drawer's floor already derived from the root font size, one wall
would move with the UI scale and two would not, and a hard wall makes that
mismatch easy to feel. Deriving them costs the narrow-container case above, which
is accepted and handled.
