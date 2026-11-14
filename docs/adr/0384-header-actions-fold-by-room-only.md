# 0384: The desktop content header folds its actions by room alone; the always-fold-at-three rule is gone

- **Status**: Accepted
- **Date**: 2026-10-07

## Context

The desktop content header had two fold rules (`useHeaderActionCollapse`).
The room rule folds the two actions nearest the title once they no longer fit,
then one more per step. The second rule, `alwaysCollapseFrom: 3`, folded any set
of three or more actions into the ⋯ menu at every width. Its reasoning was that
past a couple of icons the cluster reads as a toolbar, and the menu names each
action in words.

*Find in app* gave the app view a third action. Under the second rule, Open in
new tab and Fullscreen would leave the row at every desktop width. That undid
the reason the app view had been trimmed to two.

## Decision

Room alone decides. Every action rides the row while it fits. When it does not,
the two nearest the title fold first, then one more per step. The title reserve
(`--content-side-reserve`) stays sized for two icons and the bell.

## Rationale

The user asked for this directly: keep the title reserve, prefer not folding
when there is space, and merge two, then three, as needed. The room rule
already did exactly that. The second rule hid controls the row had room for.

The reserve does not need to grow. On the clamp's middle arm a third icon
would cross the reserve, so the measurement folds two and the cluster is three
boxes again. Only where the title box sits at its span cap is there room for
all three, and there they ride.

## Consequences

- A view with three or more actions shows them all on a wide Canvas pane.
- Near the Canvas floor it folds as before, so the title clearance holds.
- The option `alwaysCollapseFrom` is deleted. It had one caller.
- Mobile is unchanged: it folds every action but a lone one, by its own rule.

## Alternatives considered

- **Raise the threshold to four.** Lets three icons ride, but the reserve then
  has to grow to four boxes to keep the title clear. That narrows every
  Canvas title and needs a wider Canvas pane floor (about 27.5rem from 22.5rem).
- **Keep folding at three.** Hides Open in new tab, Find and Fullscreen behind
  ⋯ at every desktop width, for no gain in room.
- **No Find icon on desktop.** Keeps the old rule safe, but leaves Find
  reachable only through ⌘F on desktop.
