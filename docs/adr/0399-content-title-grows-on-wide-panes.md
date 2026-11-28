# 0399: The Canvas title grows on a wide pane, out of room no action cluster needs, up to 36rem

- **Status**: Accepted
- **Date**: 2026-10-09

## Context

The Canvas pane's title and its two chevrons sit in a centred box. The box
held a fixed `--desktop-nav-span` of 20rem, shared with the Conversation pane's
brand. A narrow pane could shrink it, but a wide one never grew it. On a wide
pane a long file name ellipsized at 20rem with most of the row empty.

## Decision

The Canvas box's span grows past 20rem once the pane has room beyond
`--content-unfolded-reserve` at each end, and stops at
`--desktop-content-title-span-max` (36rem). The Conversation brand keeps 20rem.

## Rationale

- **The chevrons still hold still across navigations.** The span depends on
  the pane width alone, never on the title or the action set.
- **No action folds that rode before.** The unfolded reserve is the widest
  action set a content view shows (a file preview's six) plus the bell. The box
  only takes room past it.
- **The cap keeps the chevrons near the title.** Without it they would drift
  out to the row's ends on a very wide pane.

## Consequences

- On a wide pane the two panes' chevrons are no longer the same distance apart.
- A view that gains a seventh action can fold two into ⋯ on a wide pane, where
  the fixed span let them all ride. Raise the reserve's box count with it.

## Alternatives considered

- **Grow up to the side reserve, capped at 36rem.** Simpler, but the title
  then takes room the file preview's icons use, so they fold into ⋯ at mid widths.
- **Grow out of whatever the current actions leave.** Gives the most room, but
  the box then changes with the action set, and the chevrons move on every
  navigation. That is the bug the centred box fixed.
- **Grow without a cap.** The chevrons end up far from a short title.
