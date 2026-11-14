# 0498: The Conversation is never squeezed below its floor: a drawer or rail that cannot fit yields, drawn-only, from a derived drawn layout

- **Status**: Accepted
- **Date**: 2026-10-16
- **Amends**: [0495](0495-canvas-yields-instead-of-squeezing.md)

## Context

ADR 0495 made the Canvas yield when the split cannot hold both pane floors. It
recorded one non-goal: the thread drawer and the Home rail could still squeeze
the Conversation below its floor. Its header row then overlapped itself, with
New thread drawn over Previous thread.

The window holds three things side by side: the drawer, the split and the rail.
The drawer drew at its stored width, and nothing held it to the window. The rail
drew at a width held for both split floors, which ignored the drawer. So
nothing protected the Conversation from the two side panes.

With New thread and Home leading the row, the floors at 100% are:

- 320 for the drawer, 498 for the Conversation and 364 for the rail.
- At 125%, they are 360, 582 and 455.

The desktop layout starts at 769px. So a side pane can fail to fit even alone:
the drawer at 125% below 942px.

The user's rule is that header icons never overlap.

## Decision

The window draws a **drawn layout**, derived from the stored layout, the
window's width and the four floors (`computeDrawnLayout`,
`components/layout/splitHelpers.ts`). It generalises ADR 0495's drawn split
ratio.

- **The Conversation never yields for room.** As room shrinks, the Canvas
  yields first, then the rail, then the drawer. The drawn side panes are the
  first set that fits beside the Conversation's floor: both, the drawer alone,
  the rail alone, neither.
- **A yield is drawn-only.** It never writes `threadDrawerOpen`,
  `threadDrawerWidth`, `homeRailShown` or `homeRailWidth`. A wider window or a
  closed neighbour brings the pane back at its stored width.
- **A toggle reads what is drawn, and a tap swaps, else is disabled.** Showing
  a yielded rail closes the drawer for real, since only the rail yields to the
  drawer. Showing the drawer needs no swap: it outranks the rail. A pane that
  cannot fit even alone gets a disabled toggle. Its tooltip says the window is
  too narrow.
- **A drag pins every yield.** `beginPaneResize` pins the Canvas, the rail and
  the drawer. `endPaneResize` decides all three from the room the drag left.
- **A drawn pane is held to the window.** The drawer is held to leave the
  Conversation's floor and a drawn rail's floor. The rail is held to leave the
  drawn drawer, the Conversation's floor and the Canvas's floor. Neither draws
  below its own floor.

## Rationale

The rule is ADR 0495's, applied one level up. A pane narrower than its floor
has a header that cannot fit its controls. The Conversation is the pane the
user works in, so it is the one that never gives way.

The order follows what each pane holds. The Canvas shows what the user opened,
so it steps aside first. The rail is a board beside the work. The drawer is the
navigation that moves the Conversation, so it stays longest.

A toggle that reads the stored state would lie. It would say a pane is shown
when none is drawn, and a tap would change stored state with no visible effect.
Reading the drawn state keeps the label, the pressed state and the badge true.

A swap shows the user what their tap bought. Showing a yielded rail has one way
to make room, which is to close the drawer. That close is persisted, because
the user asked for the rail. Showing the drawer never touches the rail's stored
state, so the rail comes back when the room does.

## Consequences

- **ADR 0495's last consequence is settled.** The Conversation is drawn at least
  its floor wide from 769px, at every ui-scale, for every stored layout.
- **The rail's width hold now counts the drawn drawer.** Today's hold ignored it.
  A wide rail beside an open drawer draws narrower than before, which keeps the
  Canvas open where it used to yield. This is the one intended change with
  room to spare.
- **At large scales a side pane can be unavailable.** The drawer at 125% below
  942px, and at 175% below 1191px, has no room even alone. Its toggle is
  disabled with a reason, and a shortcut that would show it says so in a toast.
- **`drawnSplit.ts` is `drawnLayout.ts`.** Readers that describe the screen read
  `drawnDrawerShown`, `drawnDrawerWidth`, `drawnRailShown` and `drawnRailWidth`.
  Writers keep writing the stored signals.
- **The layout measures the window, not the split.** The split's width follows
  from the window and the drawn panes, so a yield cannot feed back into its own
  measurement.
- **Yields animate like a toggle.** A side pane that yields or returns for room
  puts `pane-animate` on the split, as the Canvas does.

## Alternatives considered

**Lower the Conversation's floor while the drawer is open.** The traffic lights
then sit over the drawer, so the row reserves a lead it does not need. Fixing
that shrinks the problem band but does not remove it at 150% and up. It is a
separate header-layout decision.

**An overlay mode for a pane that cannot fit.** The pane would float over the
Conversation. The user chose a disabled toggle with a reason instead.

**Yield the drawer before the rail.** The drawer moves the Conversation, and the
rail only shows a board. Losing navigation before a board would cost more.

**Rewrite the stored layout when room runs out.** The screen would be right, but
the first narrow window would lose the user's open drawer and widths. ADR 0495
rejected the same shape for the split ratio.

**Hold the drawer's width for the Canvas.** The Canvas yields beside a wide
drawer, as before. A hold there would narrow the drawer the user set, to keep a
pane that already knows how to step aside.
