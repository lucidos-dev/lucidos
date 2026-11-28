# 0573: The composer row's picker and action button keep one width per layout; lack of room only folds; one action slot; Move to Current leaves the composer

- **Status**: Accepted
- **Date**: 2026-10-25
- **Amends**: ADR 0378 (Move to Current on the composer row)
- **Amended by**: ADR 0576 (the faces follow the row's width, not the viewport)

## Context

The composer's action row resized as it went. The model picker shed faces by
width, and the action button changed width with its label: the round Send, a
text "Archive", an Apply split, "Move to Current". So the row's anchors jumped
between states, and a text button read as oversized beside the round Send.
The user settled the target over several rounds of rendered mockups.
`docs/plans/2026-10-25-composer-row-fixed-width-controls.md` makes it
executable.

## Decision

**Within one layout, the model picker and the action button each keep one
width, whatever the room or state.** Lack of room only folds members into the
⋯. The desktop layout shows the picker's full face and a fixed-width button.
Both take the `.action-btn` corner (`--radius-control`), not a pill, so the
composer's main action reads as the same kind of button as every other one.
The phone layout shows the intelligence badge alone and a round icon button.

**One action slot.** Exactly one control stands where Send stands. Every
banner action merges into it: the face is the first by a fixed order (Apply,
Archive, Set aside, Bring back, Discard), and the rest go in its menu. On a
phone, a round button with a menu opens it on every tap.

**Fixed spots.** The action button holds the right edge and the picker stands
directly left of it. Diff and the standing apply render left of the picker
and fold before it.

**Move to Current leaves the composer row.** An archived thread's composer
shows Send. The thread's ⋯ menu keeps Move to Current. This amends the
composer half of ADR 0378. Its menu half stands.

## Rationale

**A control that moves under a finger is a mis-tap.** The user reaches for the
picker and Send by position. Anything always present keeps its spot, so
nothing that comes and goes may stand between them.

**A width that depends on the state is a width that jumps.** Shedding traded
one jump for several. A fixed width spends a little room to buy a row that
holds still.

**One slot keeps the right edge one shape.** Two text buttons side by side
were the widest thing on a phone's row, and each new state added another.

**Move to Current is navigation, not a reply.** An archived thread's composer
is for writing to it. The ⋯ menu already offers the move.

## Consequences

- A long model name ends in an ellipsis, and the tooltip names it in full.
- Every face label must fit the fixed button. Long wording moves to the tooltip
  and the accessible name ("Landing", not "Landing in a batch").
- On a phone, a split action costs one extra tap. The user preferred that to
  a caret badge too small to hit.

## Alternatives considered

- **Keep shedding, tune the levels.** Lost: every level is another width, so
  the anchors still jump.
- **A small caret badge on the round button.** Lost: the user found it too
  small to tap.
- **A full pill for the picker and the action button.** Lost: it was the one
  action button in the shell without the `.action-btn` corner.
- **Diff and the standing apply right of the picker.** Lost: they come and go,
  so the picker would move with them.
