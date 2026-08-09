# 0058: The thread drawer's width floor is one number for every desktop client

- **Status**: Accepted
- **Date**: 2026-08-09

## Context

The thread drawer's width floor (`computeMinDrawerWidth` in
`crates/lucidos-app/src/store/paneMinimums.ts`) is what a *clamped divider*
(ADR 0056) stops at, and it is derived rather than a constant: it adds up what
the drawer's header row costs at the live root font size.

Until now it was derived **per desktop build**, because the row genuinely
differs between them. On the packaged macOS build the header's controls reach up
into the reclaimed title-bar band, so the row starts after
`--titlebar-lights-reserve` (a fixed 80px) to clear the traffic lights; in the
browser it starts after its own `0.5rem`. Once the Threads title moved onto the
pane's own middle, a centred title had to clear the wider of the row's two ends
on **both** sides, so that lead is paid twice: 312px against 168px at a 16px
root.

Same workspace, same drawer, two different walls depending on which client it
was opened in, and a drag in the browser could leave the drawer 144px narrower
than the app would ever allow. The user asked for the web client to stop at the
width the packaged build stops at.

## Decision

`minDrawerWidth()` reads no build attribute. Every desktop client is floored at
the packaged build's lead, so the drawer's minimum is 312px at a 16px root
everywhere, scaling with the root font size as before.

`data-titlebar-overlay` keeps deciding how the row is **laid out** (it is still
what puts the lights reserve on `.threads-header`'s `padding-left` and on the
title's `--threads-title-lead`). It decides nothing about how narrow the drawer
may get.

## Rationale

A minimum is a promise about the workspace, not about the window it is being
viewed through. The drawer holds the same rows at the same width whichever
client opened it, and a user who resizes it on a laptop and then opens the same
workspace in the browser should meet the same wall.

Of the two candidate walls, only the wider one is a floor on both clients. Take
the narrower and the packaged build is back to a drawer that can be dragged into
an overflowing header, which is the bug the derived floor exists to prevent.
Take the wider and the web client's row simply has more room than it is obliged
to use: the web row still lays out at `0.5rem`, so the extra 144px goes to the
title, and nothing there is painting against a light that is not there.

## Consequences

- **The web client's drawer floor rises from 168px to 312px at default scale.**
  A drawer already persisted below the new floor is corrected on the next load
  by `clampThreadDrawerWidth`, which is the mechanism a UI-scale change already
  used.
- **`resetPaneLayout` now lands on the floor rather than `DEFAULT_DRAWER_WIDTH`
  for most users.** The reset is `max(default, floor)`, and 312 > 300 from a
  16px root up; the 300 constant only leads below 100% ui-scale. That is not
  dead code (it is what a 75% scale resets to), but it stopped being the usual
  answer.
- **The three pane minimums stop summing sooner.** They now total 972px at 100%
  ui-scale, 1175 at 125%, 1378 at 150% and 1581 at 175%, so on a 1280px screen
  `clampToRange`'s empty-range branch is reached from 150% instead of 175%
  (137.5%, the step below, still fits at 1277). ADR 0056's consequence section
  says "somewhere past 150% ui-scale"; this is the number that replaces it. That
  branch was already load-bearing rather than defensive, and it is now met by
  more configurations.
- **The floor overcounts what the web row needs, deliberately.** It reads as a
  bug to anyone who checks the arithmetic against the browser's own row, which
  is why it is recorded here and stated at the function.

## Alternatives considered

**Keep the floor per build.** What we had. It is the honest answer to "what does
*this* row need", and it is exactly what the user rejected: it makes the wall a
property of the client rather than of the workspace.

**Give the web row the lights reserve too**, so the floor stays "exactly what
the row needs" and the two builds converge on geometry rather than on a number.
Rejected: it would indent a browser header by 80px to clear chrome that is not
there, and narrow the web title by the width of lights it does not have. The
CSS already says so at `:root[data-titlebar-overlay] .threads-header-title`.

**Floor both clients at the web number** (168px) and let the packaged build
clamp its title instead. Rejected: at 312px the title has 4.5rem to stay a
title, and below that the drawer stops at a width where the thing it reserved
room for is an ellipsis. That is the regression the derived floor replaced.

**Set a minimum window width on the packaged build instead**, and leave the
drawer alone. Rejected as an answer to a different question: `tauri.conf.json`
sets no `minWidth` today, a browser tab has no equivalent, and a window floor
says nothing about where a divider inside it may rest.
