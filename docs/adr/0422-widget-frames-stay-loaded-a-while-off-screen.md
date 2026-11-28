# 0422: A widget frame mounts on first sight and stays loaded two minutes off screen; the shared cap still evicts

- **Status**: Accepted
- **Date**: 2026-10-10

## Context

A widget frame in the transcript mounted only while it was on screen (ADR 0407,
restated in ADR 0415). Each frame is a renderer process of its own (ADR 0227),
so unloading off screen kept a long thread cheap.

On an iPhone that made the sound player reload every time the reader scrolled
back to it. The load cover faded in again and a playing clip stopped. Answering
a question was enough: the answer glides to the live edge, the next card pushes
the player off screen, and scrolling back up reloads it.

ADR 0415 added a shared cap of 8 mounted widget frames, which evicts the frame
furthest from view. That cap bounds the cost of held frames on its own.

## Decision

A widget frame mounts the first time it comes on screen. When it scrolls off,
it stays loaded for `OFF_SCREEN_HOLD_MS` (two minutes, in `WidgetFrame.tsx`),
then unloads. The shared cap still applies and can evict a frame sooner.

## Rationale

- **Scrolling back finds the widget as it was.** A reader who scrolls away to
  read and comes back hears the clip still playing and sees no reload.
- **The cap already bounds the cost.** At most 8 frames stay mounted, so the
  hold adds no new worst case.
- **The hold bounds the time.** A widget the reader has left for good does not
  hold a renderer process for the rest of the session.
- **A frame never loads unseen.** A long thread opened at its end still loads
  no widget above the fold.

## Consequences

- Supersedes the on-screen bullet in ADR 0407's Consequences and its
  restatement in ADR 0415.
- An evicted frame stays unloaded until it comes back on screen, then mounts
  again.
- A clip longer than the hold stops two minutes after its player scrolls off.
- The shelf is unchanged: it mounts at once.

## Alternatives considered

- **Unload on scroll-off (before).** It caused the reloads above.
- **Stay loaded until the cap evicts.** The user's first pick. No timer, but a
  widget left behind holds a renderer process for the whole session.
- **Keep only a widget in use.** Hold a frame the reader pressed or one that is
  playing. The host cannot see inside a frame without an SDK signal, and an
  idle widget would still reload. The user declined it.
