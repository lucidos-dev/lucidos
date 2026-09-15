# 0178: A window is born on the display its frame names, and a placement moves before it resizes

- **Status**: Accepted
- **Date**: 2026-09-09

## Context

A user opened a workspace whose window had last been left on the other display.
The window came up at the right size, in the right place, with the page drawn
at half size in its top-left quarter. The desk is the one ADR 0173 was reported
on: a 5120x1440 ultrawide at scale factor 1.0 as primary, and the internal
3456x2234 Retina panel at 2.0.

The frame was right. `osascript` reported the live window at 1829x1084 points at
1763,1473, exactly what `.window-session.json` held. Only the webview inside it
was wrong, and wrong by exactly the 1.0 to 2.0 ratio.

Four facts compose into that, three of them in dependencies:

**The app window's own webview is a CHILD webview.** `crates/lucidos-app`
enables `tauri/unstable`, which it needs for `Window::add_child` and therefore
for the URL preview. With that feature `tauri-runtime-wry` builds every window's
own webview as `WebviewKind::WindowChild`, so wry takes `build_as_child`. On
macOS that path sets `NSAutoresizingMaskOptions::ViewMinYMargin`: fixed size,
pinned to the top of the content view. The webview does not follow the window.
Its frame is only ever what the runtime writes.

**The runtime writes it from a resize event, in physical pixels.**
`TaoWindowEvent::Resized` carries a physical size, and the handler converts it
back with `window.scale_factor()` read at that moment:

```rust
let size = size.to_logical::<f32>(window.scale_factor());
webview.set_bounds(/* size x the stored rate, which is 1.0 for a window's own webview */);
```

**The mint and the read are separated by a run-loop turn.** tao queues window
events and drains them in `cleared()`. Its own setters are deferred too:
`set_inner_size` and `set_outer_position` both go out through
`DispatchQueue::main().exec_async`.

**`place_window` sized and then positioned.** So in one turn: `setContentSize:`
fired `windowDidResize:` while the window was still on the 1.0 ultrawide, and
tao minted `Resized(1829x1084 physical)`. `setFrameTopLeftPoint:` then moved the
window onto the 2.0 panel. The drain divided that stale payload by 2.0 and set
the webview's frame to 914x542, anchored top-left.

`open_app_window` is what put the window in that position. It built every extra
window with `.inner_size(1024.0, 768.0)` and no position, so macOS birthed it on
the primary, and `place_window` moved it afterwards.

## Decision

**A window is born on the display its frame names.** A remembered frame goes to
the builder, as a logical position and a logical size. It no longer goes to
`place_window` after the window and its webview already exist.

**A placement moves before it resizes.** `app_window::place_window` is the one
routine that puts a window at a rect, and `placement_steps` is the value that
fixes the order. `window_restore::clamp_restored_geometry` applies its
correction through it rather than through a setter pair of its own.

A `ScaleFactorChanged` handler re-asserts that an app window's own webview fills
its window. That is the net, not the fix.

## Rationale

**A window that never changes display cannot mis-convert.** tao creates the
NSWindow with `initWithContentRect:` at the requested rect, so the window opens
on the target display. It carries that display's backing scale factor from the
first frame, and wry computes the webview's initial bounds from the same factor.
There is no move, so there is no resize to read at a second factor.

**The interval between the mint and the read is the whole defect.** tao reads
the live backing scale factor in `emit_resize_event`, and the runtime reads the
live one again at drain time. A resize that itself crosses displays is therefore
already correct: the frame is set before `windowDidResize:` fires, so both reads
see the destination's factor. Only a resize followed by a separate move can
straddle, and moving first removes the only pair that does.

**The order has to be a value, or it is invisible.** Two straight-line calls in
the right order read exactly like two in the wrong order. The cost of the wrong
one is a page nobody can use. `placement_steps` returns the pair so a test off
macOS can assert it.

**One placer, so the order holds.** The clamp had its own `set_size` plus
`set_position`, in the broken order, and its correction can send a window to
another display. A second pair is a second place to get this wrong.

**The builder and the placer cannot drift.** Both speak logical points, and tao
routes each through the same `util::window_position` flip.
`titleBarStyle: "Overlay"` makes the content rect the frame, so `inner_size` and
`set_size` mean one thing. That is what makes it safe for `main` to keep using
the placer while an extra window uses the builder.

## Consequences

- Every path that opens a window with a remembered frame is covered by the
  builder change: the picker row, the in-app switcher, the notifications group,
  a native banner tap, and the launch restore. All five funnel through
  `open_app_window`.
- The two paths that still place an existing window are covered by the order:
  `window_persist::size_main_window_for_its_workspace` and the clamp.
- **The clamp starts doing its job on the new-window path.** It used to run
  straight after `place_window`, whose setters had not landed yet, so it judged
  the geometry the window still had. The builder applies the frame when it
  creates the NSWindow, so the clamp now reads the real thing. On the other
  paths it still reads pre-placement geometry; `docs/known-gaps.md` carries
  that.
- The no-jump guarantee is stronger, not weaker. A restored window used to exist
  at the default geometry for the moment before the placement landed. It is now
  never anything but its own frame, and it is still hidden until shown.
- `1024x768` is named once, as `DEFAULT_WIDTH_POINTS` and
  `DEFAULT_HEIGHT_POINTS`, and `the_default_size_is_the_declared_one` pins it to
  `tauri.conf.json`.
- A URL preview is unaffected either way. It is built without `auto_resize`, so
  its `bounds` rates are `None` and the runtime's resize handler skips it.
- Nothing here changes what the session record stores or means. ADR 0173 still
  owns the units, and this is downstream of it.
- ADR 0140 still holds: the refit takes the webview flavour, and the placer
  takes the window flavour.

## Alternatives considered

**Handle `ScaleFactorChanged` and leave the placement alone.** This was the
reported hypothesis, and it does repair the damage. It repairs it AFTER the
fact, though, and only when the queue happens to drain the resize first. It also
leaves the window born on the wrong display, which is the thing that made the
conversion straddle at all. Kept as the net, for a scale change the client did
not cause.

**Reassert the webview bounds after every placement.** The placement itself is
deferred, so there is no point in our own code that runs after AppKit has
applied it. Anything we scheduled would be a guess about timing.

**Take the resize job off the runtime with `set_auto_resize(false)` and size the
webview ourselves on every `Resized`.** It is airtight against any ordering,
because we would read the live frame rather than a queued payload. It also makes
us responsible for a job the runtime does everywhere. One missed event is then a
webview that stops tracking its window for good, which is worse and quieter than
the fault being fixed.

**Drop `tauri/unstable`, so the webview is `WindowContent` and autoresizes.** On
macOS a non-child webview gets `ViewWidthSizable | ViewHeightSizable` and would
track the window with no event at all. The feature is what `Window::add_child`
needs, and that is the URL preview. Giving up the preview to avoid one
conversion is not a trade.

**Keep sizing before moving and convert the payload ourselves.** There is no
seam to do it in. The conversion happens inside `tauri-runtime-wry`, between
tao's queue and wry's `setFrame:`, and nothing in our crate is called in
between.
