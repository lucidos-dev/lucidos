# 0393: The thread drawer groups by Folders or by Status, and the status groups are an accordion

- **Status**: Accepted, amended by [0394](0394-drawer-ongoing-grouping-fixed-order.md) (the grouping is Ongoing, in one order, with no Idle)
- **Date**: 2026-10-08
- **Replaces**: the drawer view selector's status list (ADR 0359 keeps its transition)

## Context

A tester asked whether the Archive list could collapse. Every drawer section
already collapsed on a header click, but the headers were drawn flat on
purpose, with no caret, so nobody could tell.

A chevron on the collapsible headers alone would split the drawer's look. The
four status views, picked in the filter panel, each drew one plain header that
could not collapse. So the drawer needed one header shape for both ways of
looking at the list.

## Decision

- **Two groupings**, picked on a title band above the list with the words
  `Folders` and `Status`.
  - Folders is the Pinned / Current / Archive list, unchanged.
  - Status shows Needs attention, Review, In flight, Drafts and Idle at once,
    as collapsible groups.
- **Status is an accordion.** At most one group is open, and it renders last,
  so the closed headers stack above it and stay in view while its rows scroll.
  An empty group shows with a 0 and never opens. *Superseded 2026-10-09
  (ADR 0396): an empty group is not drawn.*
- **A thread shows in every group it matches.** Only one group is open, so no
  row shows twice on screen.
- **Idle** is the listed Current and Pinned threads matching no other group,
  and carries Archive all.
- **Every collapsible header leads with a chevron** that turns with the
  collapse, in the drawer and in the Triggers, Changes and Thread queue panels.
- **The band is the thread title row**, from the same rules. On the phone it
  sticks under the header with the status headers, and glides with the dynamic
  bars as the thread title does. *Superseded 2026-10-09 (ADR 0396): one header
  button swaps the grouping, and the band is gone.*
- **The filter shapes Folders only.** Its panel keeps the thread types and
  Include deleted. Its button shows only under Folders, and the
  needs-attention count moves onto the band's Status word.

## Rationale

Status and lifecycle are two questions about the same threads: "what needs me"
and "where does it live". Picking one status at a time hid the other four, and
the drawer then read as a stack of filters. All five groups on one screen
answer the first question at a glance, and the counts sit on the headers.

The accordion keeps that screen short. Several open groups would push the
closed headers off screen, which is what made a status hard to find. With the
open group last, every header stays put above the rows and one tap switches
group.

Allowing a thread in several groups keeps each group honest: its count is its
predicate's count, and the badge agrees with it. Forcing disjoint groups would
have meant a precedence rule the user cannot see.

## Consequences

- The status list, the All statuses row and the "or" rule left the filter
  panel. A device that had a status picked lands on Folders once.
- Under Status the thread-type filter does not apply, so its button shrinks
  away rather than dimming.
- On the phone with dynamic bars on, the status headers glide away with the
  band. A scroll up brings them back, as it does every other bar.
- The status predicates and `display_section()` are unchanged.

## Alternatives considered

- **"Types" as the word for the lifecycle grouping.** Rejected: it clashes with
  the filter panel's "By thread types" heading.
- **Independent collapse under Status**, as under Folders. Rejected: with two
  long groups open, the headers below them scroll away and the grouping loses
  its at-a-glance view.
- **Hiding empty status groups.** Rejected: the header order would shift as
  threads come and go, and a 0 is itself an answer. *Reversed by ADR 0396.*
- **Keeping the status picker in the filter panel** and adding chevrons only to
  the lifecycle headers. Rejected: two header shapes in one drawer, which is
  the inconsistency that started this.
- **A toggle in the filter panel, or a header icon beside Filter.** Rejected
  with the mockups: both hide the grouping behind a tap, where the band says it
  on screen. *Reversed by ADR 0396: the header title now says it on screen.*
