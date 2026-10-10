# 0333: On a phone the thread title is its own menu button; the title row drops its pin and ⋯, the drawer keeps its pin

- **Status**: Accepted
- **Date**: 2026-09-30

## Context

On a phone the thread title row ended in two icon buttons, the thread pin and
the thread ⋯ menu. Both sat against the pane's right edge, the hardest reach
on a phone. The drawer rows had already dropped their ⋯ on mobile for a long
press (`docs/plans/2026-08-29-mobile-thread-row-long-press-actions.md`). That
plan kept both controls on the two title rows.

## Decision

On a phone the title itself is the menu button. A tap toggles the thread menu
and a long press opens it. The menu is the existing popover, aligned to the
title's leading edge. The title row draws no pin and no ⋯, and no cue glyph.
The thread menu shows Pin/Unpin on every open.

The desktop title row keeps its pin and ⋯. The drawer rows keep their pin on
both layouts.

## Rationale

- **A tap on a title is how people find a title menu.** A long press alone is
  a hidden gesture, and the tap needs no hint. The hold stays as the
  accelerator an iOS user expects, and it matches the drawer rows.
- **One real control serves every input.** A native `<button
  aria-haspopup="menu">` named by the title is what VoiceOver announces and a
  keyboard tabs to. No hidden duplicate control is needed.
- **The drawer's pin carries information the title's did not.** In a list, a
  filled or outline pin shows which threads are pinned at a glance. Inside a
  thread, the Pinned section and the menu's own label say the same thing.
- **The menu must list every action.** With no pin on the title row, Pin/Unpin
  in the menu is the only route there. Showing it on every open, everywhere,
  keeps one menu shape instead of a per-surface rule.

## Consequences

- One-tap pinning from inside a thread becomes tap, then tap. One-tap pinning
  stays on every drawer row.
- A hold on the title selects no text. Copy thread title in the menu stands in.
- The title's first letters sit inside the left-edge swipe guard. As a button
  they are exempt from it, like every edge control, so iOS's own back swipe may
  start there in the installed app.
- While the keyboard is up, the title stays tappable, as the pin and ⋯ were.
- `OverflowMenu` takes a trigger face: the host's content drawn as the menu
  button. A face and a host opener are exclusive in its type.

## Alternatives considered

- **Long press only, taught by a one-time cue.** Rejected: the cue needs a
  stored "seen" flag per device, and VoiceOver would still need a hidden
  button. The tap does the teaching for free.
- **A chevron after the title.** Rendered and rejected by the maintainer: the
  plain title reads better, and the tap is discoverable without it.
- **A menu attached to the row, as a full-width sheet.** Rejected in favour of
  the existing popover, which every other thread menu already uses.
- **Drop the drawer's pin too.** Rejected: there the pin is also the pinned
  state across the list, which the title row never needed.
- **Show Pin in the menu only where no pin button sits beside it.** Rejected:
  a per-surface rule for one item, where showing it always costs a repeat of a
  button that is right there.

## Amendment

**The desktop title is a menu button too.** The maintainer asked for it on
2026-09-30. A click toggles the thread menu, and a right-click opens it at the
pointer. The cursor shows the title is clickable. The desktop title row keeps
its pin and drops its ⋯, since the title is the menu button.

The desktop drawer rows keep their ⋯. A left-click on a row opens the thread,
so without the ⋯ only right-click reaches the row menu, and nothing on screen
shows it. The Open thread actions shortcut also works through that ⋯.
