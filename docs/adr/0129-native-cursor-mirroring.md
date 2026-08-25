# 0129: Mirror the page's CSS cursor onto the native window

- **Status**: Accepted
- **Date**: 2026-08-25

## Context

In the packaged macOS app the cursor over a pane divider was unstable, and read
as a plain arrow almost always under trackpad movement. The drag worked, so hit
testing was right and only the glyph was wrong.

`tao` gives its content view a cursor rect spanning the whole view, carrying the
window's own cursor icon, which defaults to the arrow
(`platform_impl/macos/view.rs`, `reset_cursor_rects`). AppKit re-asserts a cursor
rect as the mouse moves inside it. WebKit sets `NSCursor` straight from the CSS
`cursor` property on the same moves. Two writers, one cursor, so whichever ran
last for a given move wins. A trackpad reports far more movement than a mouse,
which is why it looked broken there and merely twitchy elsewhere.

Upstream tracks this as wry #175 and tao #386, both open and labelled blocked on
upstream.

## Decision

The page tells the native window what the hovered element asks for, so both
writers write the same thing. One document-level `pointerover` listener reads
`getComputedStyle(target).cursor` and forwards the keyword over a
`set_window_cursor` app command, which resolves it to a `tao::CursorIcon`.

It covers every element, not the dividers alone: the same race governs a
button's hand cursor and a text field's I-beam.

## Rationale

Nothing on the CSS side helps. The race is between AppKit and WebKit, and a
stylesheet is not a party to it. `Window::set_cursor_icon` writes the very icon
tao's rect reads, so pointing it at the hovered element's cursor removes the
disagreement rather than trying to win it.

Three things follow from the shape, and each is a decision:

- **A reconciler, not paired enter and leave handlers.** One listener answers
  for whatever is under the pointer now. tao's rect spans the window, so a claim
  stranded by a missed leave would be one wrong cursor over the whole app.
- **One table, in Rust.** The page holds no keyword list at all and forwards
  what the browser computed. Two tables would be free to drift, and a keyword
  missing from one of them falls back to the arrow, silently.
- **The table is total over the CSS keyword set.** All 36 keywords the property
  accepts have a `CursorIcon`, so no stylesheet can produce one it lacks. A
  source scan (`utils/nativeCursor.drift.test.ts`) proves that against our own
  CSS rather than trusting it.

## Consequences

- The bridge carries at most one call per pointer boundary crossing, and
  de-duplicates, so a run of elements that agree costs one call.
- `cursor: auto` over selectable text keeps the race. `auto` is the initial
  value, and WebKit resolves it per hit test to an I-beam or an arrow. The
  computed value is still `auto`, so the page cannot tell which, and it maps to
  the arrow. Resolving it would need a `caretRangeFromPoint` hit test, which
  forces layout on a pointer path.
- Cursors inside an app or preview iframe are not mirrored, since a pointer
  event there never reaches the host document.
- The IPC surface grows by one command, inside the existing `allow-app-ipc`
  permission.

This is a workaround with an owner and a trigger, so it is registered in
`docs/temporary-measures.md` § "Native cursor mirroring".

## Alternatives considered

**Disable cursor rects on the `NSWindow`.** One `disableCursorRects` call at
window creation kills tao's rect outright, needs no page-side work, and costs
nothing at runtime. It also turns off cursor rect management for the window's
own frame, so the edge resize cursors may go with it. That cannot be verified
from a worktree, since the packaged app is not launchable there. Rejected as the
blunter of two fixes, not as a wrong one: revisit it if the reconciler proves
noisy.

**Scope the mirroring to the dividers, through an opt-in attribute.** A smaller
surface, and it fixes the reported symptom. It leaves every button and field in
the race. The mechanism is the same either way, so the narrower version only
defers the rest of the work.

**Widen the divider's grab zone.** Fixes a different problem. The drag already
worked, and an overhang would cover the transcript's scrollbar wherever classic
scrollbars are on.

**Re-assert the CSS cursor on a timer while hovering.** WebKit re-evaluates on a
style change, so alternating between two spellings would keep it winning. It
burns a frame budget to lose a race by less, and there is no second spelling of
`col-resize`.
