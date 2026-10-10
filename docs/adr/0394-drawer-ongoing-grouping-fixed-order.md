# 0394: The drawer's second grouping is Ongoing: one fixed order, no Idle, and Show in thread list stays in it

- **Status**: Accepted. Amended by [0401](0401-drawer-ongoing-tile-selector.md) (tiles replace the accordion), [0410](0410-show-in-thread-list-opens-folders.md) (renamed Show in Folders, and always opens Folders) and [0409](0409-review-lists-read-requests.md) (Needs attention renamed Blocked, new tile order)
- **Date**: 2026-10-08
- **Amends**: [0393](0393-drawer-folders-status-grouping.md)

## Context

The first build of ADR 0393 was used for a day, and three of its choices
read badly in practice.

- The open group moved to the bottom. The headers then changed order with
  every tap, which the user called "too clever".
- Idle listed the quiet Current and Pinned threads. Without an Archived group
  beside it, Idle was half of a lifecycle view inside a status view.
- "Status" named the mechanism, not what the list shows.

## Decision

- **The grouping is called Ongoing**, in every layer: the band word, the
  `DrawerGrouping` value, the group types and the persisted keys.
- **Its groups keep one order**: Drafts, Needs attention, Review, In flight.
  It stays an accordion. The headers down to the open group stick under the
  band. The later headers follow its rows, so a long group is folded to reach
  them.
- **There is no Idle group.** A thread that matches no group is listed under
  Folders only. Archive all lives on the Current header.
- **Show in thread list stays in Ongoing** when a group lists the thread. It
  keeps the open group if that group lists it, and otherwise opens the first
  that does. Only a thread in no group switches to Folders.

## Rationale

A fixed order is learnable. The eye goes to the same place for Review every
time, and that beats keeping every header in view.

Ongoing lists what is still going on. Quiet threads belong to the place they
live, which is what Folders is for. Merging the two views was weighed and
rejected: lifecycle and status answer different questions.

Drafts leads because a half-written message is the user's own unfinished
work. It is the cheapest thing to finish.

## Consequences

- A device that stored the Status grouping or the Idle group lands on Folders,
  with no group open, once.
- The thread filter's panel keeps only the thread types and Include deleted.
  Its "By thread types" heading went with this change, since the pane title
  already says Filters.
- The Filter button now fades in place under Ongoing, in a slot that keeps its
  box, rather than shrinking. Nothing beside it moves, so the fade leaves no
  ghost (`.claude/rules/frontend.md`, a single control).
- When the open group moves above another, the header that closes changes
  parent, from the sticky block to after the rows. It remounts, so its chevron
  snaps rather than turning on that one transition.

## Alternatives considered

- **Keep the open group last.** Rejected: the order changes on every tap.
- **Sticky headers for every group**, the later ones at the bottom. Rejected:
  a long open group would leave a short window for its rows on a phone.
- **Add an Archived group, or merge Ongoing into Folders.** Rejected: two
  questions in one list.
- **Other words**, each taken:
  - "Desk": *Desk fit*, the attached displays.
  - "In progress": reads as the In flight group.
  - "Watching": an event wait.
  - "Working on": the Working status.
  - "Active": the Active indicator.
  - "Pending": a pending change.
  - "Aktuelt": "Current", a Folders section.
