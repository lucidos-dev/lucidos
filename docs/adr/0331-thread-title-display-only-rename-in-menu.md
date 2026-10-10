# 0331: The thread title is display-only; rename lives in the thread menu

- **Status**: Accepted
- **Date**: 2026-09-29

## Context

The thread pane drew its title in a band under the header. The title was also
an inline editor: a tap opened a text field, with a model-suggested name
under it. On desktop the band repeated what the drawer already showed, since
the drawer highlights the open thread's row.

The maintainer asked to cut the band down. The first idea was moving the
title into the Lucidos menu. It was dropped because a tap to see the title is
worse than the glance it replaced. The design that shipped came out of that
discussion. Plan: `docs/plans/2026-09-29-thread-title-rename-in-menu.md`.

## Decision

The title only displays. It reads like a drawer row: the status dot starts its
first line, and the pin and ⋯ centre on that line. Rename… and Suggest name are
items in the thread ⋯ menu. Rename opens the prompt dialog prefilled with the
title, and F2 opens the same dialog. Suggest name offers a name in a toast and
changes nothing until the user takes it.

The title row shows on both layouts, whether or not the thread drawer is open.
On both, its tinted band shows only once the transcript scrolls, with no
hairline. It first rolled shut on desktop while the drawer was open; see
§ Amendment for why that was reversed.

## Rationale

- **One rename path.** On desktop the title may not be on screen at all, so an
  inline editor cannot be the only way in. The ⋯ menu is on the title row, the
  desktop drawer row and the mobile long press, so one path reaches all three.
- **The drawer already names the thread.** Reversed, see § Amendment. The
  argument was that with the drawer open, a second title only costs a row.
- **The band belongs to scrolling.** At rest there is nothing under the title
  to separate from. Once content scrolls under it, the band makes the title
  leave with the header as one bar. Without it, the title drifts away like
  loose transcript text.
- **Other chat apps split the same way.** ChatGPT shows no title in the open
  chat, and users ask for it back on mobile. So mobile keeps its title row.

## Consequences

- Renaming takes one more tap than before: the ⋯, then Rename….
- As first shipped, the open thread could have no title anywhere. That
  happened with the desktop drawer open on another view, or with its row folded
  or filtered out, and the ⋯ went with the title. The amendment below fixed it.
- The prompt dialog gained the IME guard the inline editor had. An Enter that
  commits an IME candidate no longer submits any prompt.

## Alternatives considered

- **The title in the Lucidos menu on mobile, none on desktop.** A tap to learn
  which thread you are in is worse than a glance. Swiping to the drawer costs
  no more and was already there.
- **Drop the title everywhere and rely on the drawer.** The drawer does not
  always show the open thread: the thread filter, a folded family or a scroll
  can each hide its row. Mobile users often land in a thread from a
  notification and need to know which one.
- **Keep tap-to-rename beside the menu items.** Offered at plan approval and
  declined. Two ways in, and one of them missing whenever the title is hidden.
- **No band at all.** Rendered and rejected: mid-slide the title reads as
  transcript text moving at the wrong speed.

## Amendment

**The desktop title row stays while the drawer is open.** The maintainer
reversed the roll-shut on 2026-09-30. The second consequence above was the
reason: the open thread's drawer row is often scrolled, folded or filtered out
of view. The title row is then the only place that names the thread. It also
carries the ⋯ with Show in thread list, which is how the user finds that row
again. The rationale "the drawer already names the thread" held only while the
row was in view.

The rest of this decision stands: the title is display-only, and rename lives
in the thread menu. The title opens that menu on both layouts (ADR 0333), and
never edits in place.
