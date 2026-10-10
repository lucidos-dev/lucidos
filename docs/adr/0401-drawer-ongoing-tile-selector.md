# 0401: Ongoing picks its group from four always-drawn tiles; only the selected group lists below

- **Status**: Accepted
- **Date**: 2026-10-09
- **Supersedes**: the accordion of ADR 0394, and ADR 0396's "Ongoing draws only
  the groups that hold a thread"

## Context

The Ongoing grouping drew Drafts, Needs attention, Review and In flight as an
accordion, with at most one group open (ADR 0394). The headers below the open
group came after its rows. A long open group pushed them out of reach, and
reaching them meant folding the group first. The user asked for the status
selection on top and the selected status's threads always below it, in the
same Ongoing view. Folders stays its own view.

## Decision

Ongoing draws a 2x2 grid of tiles, one per group, in the fixed order. Each tile
shows its count, icon and name. Exactly one group is selected, and only its
threads list below the grid. All four tiles always draw. An empty tile is
dimmed but selectable, and a selected group that empties stays selected and
says so.

## Rationale

- **Every group is one tap away.** The selector never moves, whatever a
  group holds, so no group is ever folded out of reach.
- **A stable grid.** Hiding empty tiles would reflow the grid under the
  pointer as threads come and go. ADR 0396 hid empty headers because a header
  with no rows was a dead control. A tile with a 0 is not: it is a count you
  can read at a glance, and selecting it shows the empty state.
- **The selection stays put.** Jumping to another tile when the selected
  group empties would move the list under the reader. The empty state says
  what happened instead.
- **A quiet selected state.** The user found a full accent fill too loud. The
  selected tile lifts to a lighter neutral, and only its count and icon take
  the accent, so every theme stays calm.

## Consequences

- `openOngoingGroup` became `selectedOngoingGroup`, never null, under a new
  storage key. A stored "open group" is ignored, so each device starts on the
  default pick once.
- The grouping button's entry pick (`ongoingGroupOnSwitch`) now always names a
  group: Needs attention when badged, else the stored group while it has
  threads, else the first group with threads.
- Keyboard: ↑/↓ walks the tiles, then the rows. ←/→ steps along the tiles and
  Enter presses one, as a click does. ← from a row returns to its tile.
- A press also opens the group's first thread, the selected tile included. The
  user asked for it later. On the phone it opens only a sole thread: its drawer
  is its own pane, and opening one of several would swipe the reader off the
  list.
- The In flight running/waiting split moved from its header into its tile.
- "Nothing ongoing" is gone. Each group has its own empty state.

## Alternatives considered

- **Keep the accordion, with every header pinned on top.** It also solves the
  reach problem. But four header rows cost the most height of the rendered
  options. And a close gesture means nothing once one group always shows.
- **One row of icon chips.** The smallest footprint, but the group names drop
  to tooltips.
- **One row of count tabs.** Readable and compact, but the user picked the
  tiles when shown all four side by side.
- **Merge Ongoing into the Folders view**, groups above the folder list. The
  user ruled it out: the two stay separate views.
