# 0202: A page fills its window because we assert it, not because a stored rate says so

- **Status**: Accepted
- **Date**: 2026-09-17

## Context

A user updated the packaged client. One of two windows came up with its page
clipped on the right and a strip of desktop below it. Moving it, resizing it,
restarting the engine and relaunching the app all left it exactly as it was. The
other window, on the other display, was fine. The same shape then appeared on a
second window, produced by a resize.

Reading the live accessibility tree settled what it was:

| window | window frame | its webview |
|---|---|---|
| the broken one | 2560x1410 | **4297x1410** |
| the healthy one | 3267x1410 | 3267x1410 |

Shrinking the broken window by 120x80 and restoring it:

```
before:   window 2560x1410   webview 4297x1410
shrunk:   window 2440x1330   webview 4096x1330
restored: window 2560x1410   webview 4297x1410
```

So the page does follow the window. It follows it at 1.6786 times the width and
1.0 times the height, and holds that ratio exactly.

**That ratio is a value the runtime stores.** `tauri-runtime-wry` keeps a
`WebviewBounds { x_rate, y_rate, width_rate, height_rate }` per webview, and on
every `TaoWindowEvent::Resized` sets the webview to `window_size * rate`. An app
window's own webview is born at `(0, 0, 1, 1)`. It has to be: the crate enables
`tauri/unstable`, so that webview is a `WebviewKind::WindowChild` and does not
autoresize on its own (ADR 0140, ADR 0178).

The rate is rewritten by `SetBounds`, and `app_window::refit_webview` is the
client's only caller. The handler recomputes it as the bounds it was passed over
`window.inner_size()`, read from the tao window, which is the content view.

**`tauri::Window::inner_size()` does not answer that question.** On macOS, for a
window with no `add_child` webview, the runtime returns the first WEBVIEW's
NSView frame instead. Deliberately, and the source says why: "resized event from
tao doesn't include a reliable size on macOS because wry replaces the NSView."
It reports the PAGE.

So `refit_webview` asked the page how big the window was, got 4297, and set the
page's bounds to 4297, which changed nothing. The runtime then divided that by
the real window, 2560, and stored 1.6786 as the rate. **1.6786 is 4297 / 2560**,
and the height matched, so `height_rate` came out exactly 1.

Nothing re-derives the rate, so a wrong one is permanent for that window. It
survives a move, a resize, an engine restart and an app relaunch. The only thing
that cleared it was opening a new window.

Three more callers believed the same getter, and each inherited the same lie.
`clamp_restored_geometry` judged the page's frame and called it the window's.
`persist_window_session` wrote the page's size into the session record as the
window's, which is how a drifted frame survived a relaunch. `panel_preview`'s
`title_bar_gap` measured the gap between the page and itself.

## Decision

**Nothing asks a window for its inner size.**
`app_window::window_content_size` reads `outer_size()`, the NSWindow frame, and
is the only reader. All four callers go through it, and
`no_window_is_asked_for_its_inner_size` fails the build on a fifth.

**A page filling its window is asserted, not inferred.** `refit_webview` reads
the page's size and position as well as the window's, and returns early when
they already agree. It runs on every `Resized` for an app window, not on a scale
change alone. It runs once more before the show at startup.

**`main`'s geometry is settled once, from the startup show.** `StartupStep`
becomes Geometry, Refit, Show. The placement leaves `setup`.

**A frame the client CHOOSES is judged before it is written.**
`window_restore::sanitized_frame` runs `sanitize` on the rect the record names
and returns what to place. `clamp_restored_geometry` stays for the other case,
where nothing is remembered and the window wears what the plugin restored.

## Rationale

**The honest read is the fix; everything else is consequence.** Both sides of
the runtime's division now name the same thing, so the rate this records is 1.0
every time. A wrong one is no longer a thing the client can write.

**Running on every resize is therefore repair, not risk.** Each pass re-pins the
rate, so a rate poisoned by anything else, including a build before this one,
cannot outlive the next resize. That is why the user's window heals without
anybody editing a file.

**The mismatch gate is a saving, and an honest one only on a settled window.** A
live drag writes each frame: `refit_webview` runs from `on_window_event`, which
the runtime calls before its own auto-resize, so the page read there is always
the previous frame's. The cost is one `setFrame:` beside the one AppKit is doing
anyway, and it lands the right size a turn earlier than the runtime would.

**A rate is the wrong mechanism for an absolute invariant.** The client says an
app window is exactly one webview covering the whole window, with nothing
insetting it. A rate is a derived number the client never reads and cannot
verify, and one bad derivation is forever. Reading the property back and
correcting it is what makes the claim true rather than hoped for.

**A wrapper for a one-line getter earns its place here.** The call sites all
read correctly: the name says window, the type is a window, and the number is
the page's. Only a named reader can carry the reason, and only a named reader
gives the scan something to point at.

**The comparison is in physical pixels, because that is the one space both
getters answer in.** No scale factor enters the decision, so nothing in it can
be off by a conversion. The correction still goes out in logical points, per
ADR 0173.

**The position is judged, not just the size.** The runtime stores a rate per
corner as well as per axis, so a poisoned `x_rate` insets the page without
changing its size. Judging the size alone would leave that shape uncorrected.

**`setup` is the wrong place to size `main`.** It runs before `app.run()`, so
nothing issued there has reached the window: tao defers every setter to the main
dispatch queue, and so does the window-state plugin's restore. ADR 0193 already
moved the clamp out for that reason, and named the first moment a step can see
what the window will wear. The placement belongs in the same block, on a window
that is still hidden.

**Placing and clamping in one block would reopen ADR 0193's trap.** The
placement is deferred too. So a clamp issued behind it reads the geometry the
window is about to lose, and can correct a rect the placement then overwrites.
Judging the rect first removes the read entirely. It also means no bad frame is
ever written, where the clamp could only undo one that had been.

**Geometry is one step, because it is one question with two answers.** A
remembered frame is the client choosing; no frame is the plugin having chosen.
Running both would be the client judging somebody else's answer and then
discarding it.

## Consequences

- A settled window costs three getters and no write. A live drag writes once a
  frame, for the reason in the Rationale.
- A window whose rate is already wrong heals on the first resize after this
  lands. No user file needs repairing.
- The session record stops recording a drifted page's size as its window's
  frame, which is what carried a bad frame across a relaunch.
- A URL preview's title-bar gap is measured against the window again. Before
  this it was the difference between the page and itself, which is zero exactly
  while the page happens to fit.
- The steady state is now checkable from outside: the window's frame and its
  webview's frame must be equal in the accessibility tree.
- `REFIT_TOLERANCE_PX` is 1. The measurement is in pixels and the correction in
  points, so a neighbouring pixel is a round trip rather than a fault. Zero
  tolerance would write on every frame of a drag, which is the thing the gate
  exists to avoid.
- The runtime keeps its own auto-resize. ADR 0178 rejected taking that over, and
  the reason stands: a missed event would be a webview that stops tracking for
  good. Our refit runs after the runtime's handler, so a missed refit degrades
  to the old behaviour rather than to a dead page.
- ADR 0178's "reassert the webview bounds" is no longer the occasional net it
  was kept as. It is standing, and it is the mechanism rather than the backstop.
  Nothing else in that ADR changes: a window is still born on the display its
  frame names, and a placement still moves before it resizes.
- A URL preview is unaffected. It is a separate child built without
  `auto_resize`, so it carries no rate and the refit takes the window's own
  label (ADR 0140).
- A launch that comes up menu-bar-only never reaches the startup show, so it
  never settles `main`. The first tray reopen does it instead, bounded to once
  by `MAIN_GEOMETRY_SETTLED`. Unbounded, a later tray click would re-place a
  window the user had dragged, and inside the save debounce it would land on the
  pre-drag rect.
- `reopen_client` takes the same shape as the startup show, through
  `settle_main_geometry`. So it no longer places and then clamps, and
  `docs/known-gaps.md` § "The restore clamp judges the geometry a window had
  before the placement" is closed.
- `policy_and_displays` is the one reader of the config and the desk. The clamp
  and `sanitized_frame` share it, so the two cannot reach different verdicts
  about the same rect.
- None of this is verifiable through a driven UI (ADR 0016). `webview_fills_window`
  is pure and covered; the wiring is `STARTUP_SHOW_STEPS` plus inspection; the
  end-to-end check is the accessibility reading above.

## Alternatives considered

**Keep the refit on `ScaleFactorChanged` alone.** What shipped, and what the
report disproves. With the dishonest read it was not a net at all: every run
recorded whatever drift the page already had as the new rate. Tested on the
reporter's machine, dragging across displays left the window exactly as broken
as it found it.

**Fix the read and leave the refit occasional.** It stops the client creating a
bad rate, and it is half the fix. It heals nothing. A window already carrying
one keeps it until a new window is opened, and anything else that ever writes a
rate is still permanent.

**Refit unconditionally, with no mismatch gate.** Barely different once the read
is honest, since a drag writes every frame either way. The gate is kept for the
settled paths: a scale change on a window nothing has resized, and the startup
step, which then cost nothing.

**Take the resize job off the runtime, `set_auto_resize(false)`, and own it.**
Rejected again for ADR 0178's reason. One missed event would be a webview that
stops tracking its window for good, which is worse and quieter than the fault
being fixed. Running after the runtime keeps its work as the floor.

**Read the `Resized` payload instead of the window.** It is the same number:
`WindowEventWrapper::parse` builds that payload through the very helper this
ADR is about, so it reports the page too.

**Have the frontend correct its own layout.** The page cannot see this. Its
viewport IS the webview, so a page 1.68 times its window is internally
consistent and has nothing to detect.

**Reset the rate with `set_auto_resize(true)` instead of writing bounds.** It
recomputes from the webview's CURRENT bounds, which are the wrong ones. It would
record the fault as the new truth.
