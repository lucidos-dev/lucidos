# 0405: A widget opens from the shelf; the transcript holds a one-line widget row, never a frame

- **Status**: Accepted. Placement amended by 0407: inline widget cards that never scroll, and an opt-in shelf
- **Date**: 2026-10-09

## Context

ADR 0402 drew a widget inline at the turn that made it, in a frame that grew to
a cap and then scrolled inside. The first real use, a flight picker on a phone,
put a scroller inside the transcript's scroller. A swipe on the widget either
scrolled the widget or the thread, and the reader could not tell which.

Products that put UI in a chat agree on one rule. OpenAI's Apps SDK UI
guidelines say an inline card is single-purpose and "cards should auto-fit
their content and prevent internal scrolling". Rich content opens fullscreen.
Baymard's research finds inline scroll areas trap users on touch screens. A
t3code bug from the same week shows inline HTML in a chat swallowing every
swipe on Android. Claude's artifacts and ChatGPT's canvas both put a small card
in the transcript and open the content in its own surface.

Plan: `docs/plans/2026-10-09-widgets-live-on-the-shelf.md`.

## Decision

A widget opens from its chip on the thread's widget shelf, under the title, or
full size in Canvas. The transcript holds a one-line widget row at the turn
that made it: icon, name and menu, and no frame. A tap on the row opens the
widget as the chip does.

This amends ADR 0402's placement only. A widget is still an app kind, with the
same storage, events, reach and lifecycle.

## Rationale

- **One scroller per surface.** The shelf drop-down covers the transcript, so
  the widget's frame is the only scroller under the finger.
- **A widget stays small.** With no inline cap to fill, the guidance can say
  what the product wants: one phone screen, no scroll area inside. A widget
  that needs more is an app.
- **The row keeps what the frame gave.** It marks where the widget was made, it
  is where "Show in thread" lands, and it brings back a widget hidden from the
  shelf.
- **Fewer renderer processes.** No frame mounts as the reader scrolls past old
  turns (ADR 0227).

## Consequences

- The widget frame mounts only on the shelf drop-down and in Canvas. The
  inline cap and the on-screen watch go.
- The agent tells the user a widget is on the shelf, not "above".
- A tap on a hidden widget's row puts its chip back and opens it.

## Alternatives considered

- **Keep the inline frame, with a smaller cap.** Still a scroller inside a
  scroller whenever the content outgrows the cap. Lost on the touch trap.
- **An inline frame that never scrolls, at full height.** A tall widget then
  pushes the conversation off screen, and every old turn mounts a renderer
  process. Lost on transcript length and cost.
- **Nothing in the transcript at all.** Simplest, but then a hidden widget has
  no way back short of asking the agent. "Show in thread" also has nowhere to
  land. Lost on Hide staying non-destructive.
