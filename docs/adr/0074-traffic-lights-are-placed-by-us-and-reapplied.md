# 0074: The macOS traffic lights are placed by us through the NSWindow and re-applied from AppKit's own resize notification, because Tauri's Resized event lands a run-loop turn too late

- **Status**: Accepted
- **Date**: 2026-08-13

## Context

Under `titleBarStyle: "Overlay"` the webview owns the full window height and the
three window buttons float above it in an AppKit layer. Two numbers are
therefore ours: the x the cluster starts at, and the y that puts its vertical
centre on the centre of our own header bar. The bar height moves with the UI
scale, which is a live preference, so neither number can be fixed at build time.

AppKit also puts everything back the way it likes it on **every** window resize,
fullscreen enter and exit included. A placement applied once does not survive
the first drag.

## Decision

Place the cluster ourselves through `Window::ns_window()`, by growing
`NSTitlebarContainerView` and setting each button's `origin.x`. Re-apply from
AppKit's own `NSWindowDidResizeNotification`, registered per window with a nil
queue so the block runs synchronously on the posting thread.

## Rationale

**No runtime setter exists.** `WebviewWindowBuilder::traffic_light_position` is
creation-time, and the `main` window is declared in `tauri.conf.json` rather
than built with that builder. One rung down,
`WindowDispatch::set_traffic_light_position` is implemented in
`tauri-runtime-wry`, but `tauri::Window` wraps no public method for it and its
dispatcher is private. The crate already drives AppKit through `objc2` for the
Dock badge and the tray, so doing the placement here costs one small function.

**The notification is both late enough and early enough**, which had to be
measured rather than reasoned about. By the time it fires AppKit has already
reverted both numbers, so there is something to correct. No later layout pass
reverts them again, so what we write is what gets committed.

**The geometry is read, not baked in.** A button's `origin.y` inside the
titlebar view is AppKit's to set, so growing the container is what moves the
cluster down. Both AppKit terms are read at the call site, so a macOS release
that retunes the titlebar keeps the cluster centred instead of drifting.

## Consequences

- One mechanism, applied at every moment that needs it. New-Window children
  deliberately do not also pass `traffic_light_position` to the builder. That
  would install wry's own `drawRect:` re-apply holding the creation-time value,
  which then fights every later push.
- `on_window_event`'s `Resized` arm stays. It covers one moment the
  notification does not: tao emits a second, synthetic resize from
  `windowDidExitFullscreen:`, after AppKit has moved the buttons back out of
  the fullscreen overlay titlebar. That one is late by construction, and late
  is right for it.
- Observers are keyed by Tauri window label and removed on `Destroyed`. A dead
  window's address can be reused, and a stale registration would then place
  lights on somebody else's window.
- The x is the single source for `--titlebar-lights-reserve`. The room the
  header row keeps clear is therefore arithmetic on the value we applied, not a
  guess at where the OS left the buttons.
- The bar height is persisted, so a cold launch places against the user's bar
  rather than the compiled default.

## Alternatives considered

- **The builder's `traffic_light_position`.** Creation-time only, and the main
  window has no creation-time hook beyond a static config literal. That literal
  could carry neither a persisted value nor the live UI scale.
- **Tauri's `Resized` event as the only re-apply.** tao's `windowDidResize:`
  does not run our handler, it calls `AppState::queue_event`, so the correction
  misses the CoreAnimation transaction the resize is committed in. Measured:
  every step of an edge drag displayed the cluster at AppKit's own centre
  before the queued event pulled it back down, and the lights danced.
- **The container's `NSViewFrameDidChangeNotification`.** Probed. It fires
  while AppKit is still descending the titlebar view tree, before it resets the
  buttons' x, so the cluster displays at AppKit's x.
- **Pinning the container with layout constraints.** Probed. It does hold the
  height across a resize, but leaves that same x behind. It buys a constraint
  fight with the theme frame and still needs a per-resize re-apply.
- **Setting each button's `origin.y` directly.** AppKit owns it and would
  fight. Growing the container is the shape wry and tao both use.
