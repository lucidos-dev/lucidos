# 0420: The phone's pane strip stays overflow: hidden; clip broke iOS text editing in the prompt box

- **Status**: Accepted
- **Date**: 2026-10-10

## Context

`.mobile-swipe-container` is the phone's pane strip. It holds the three panes
on one translated track and clips the two that are off screen. It was
`overflow: hidden` for months. A scroll into view or a `focus()` inside a pane
could then scroll the strip sideways. A `scroll` listener in
`MobileSwipeContainer.tsx` put it back a frame later.

A Playwright tap scrolls its target into view first, and one tap landed inside
that frame. So the strip moved to `overflow: clip`, which is no scroll
container at all.

The next day the maintainer reported the prompt box on an iPhone. The edit
menu (Paste, Select) mostly failed to open, and selecting and moving the caret
felt slow and odd. Nothing else in the composer's path had changed in that
window.

## Decision

The strip stays `overflow: hidden`, with the `scroll` reset as the permanent
answer to a strip that code scrolls. No `overflow: clip` on it, or on any other
ancestor of the prompt box on the phone.

## Rationale

- **The timing.** The report arrived the day after the change, about a control
  that worked before it.
- **The mechanism.** iOS WebKit hides the edit menu and the selection handles
  when it judges the focused field clipped out of view. It measures that through
  the field's clipping ancestors. Every prompt box on the phone sits in this
  strip, on a track translated by a whole pane inside a composited box.
  `hidden` there is the path WebKit has long handled. `clip` is the newer one.
- **The two costs are not alike.** The scroll reset costs one frame of offset
  after a scroll into view, which a person almost never taps inside. A broken
  edit menu costs paste and selection on every message.

No device test reproduces the edit menu, so this rests on the report and its
timing, not on a capture. If the menu still fails with `hidden`, this change was
not the cause, and the search continues elsewhere.

## Consequences

- `components/layout/__tests__/mobile-swipe-container-clip.test.ts` holds the
  strip on `hidden` and fails on `clip`.
- `e2e/pane-strip-holds-still-mobile.spec.ts` asserted `clip`'s guarantee and
  is gone. The UI-scale slider spec lets the strip settle before it taps.
- The `scroll` reset is no temporary measure now. Its registry row is closed.

## Alternatives considered

- **Keep `clip` and fix the edit menu another way.** Lost: no page-side lever
  makes iOS show its edit menu once it decides the field is clipped.
- **`clip` on desktop browsers only.** Lost: the phone layout is what has the
  strip, and the e2e projects that run it include WebKit.
