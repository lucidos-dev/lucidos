# 0549: The iOS home-screen app runs under a translucent status bar again, with a fixed 12px top strip that keeps iOS 27's blur off the header, superseding 0548

- **Status**: Superseded by [0551](0551-ios-header-icons-below-the-status-bar-blur.md)
- **Date**: 2026-10-22

## Context

ADR 0548 moved the installed iPhone app below an opaque status bar, to escape
iOS 27's blur over the top of a translucent one. The header then stopped at the
status bar and no longer filled the Dynamic Island area. The user rejected that
look: the header must run up behind the island, as before.

## Decision

The app declares `black-translucent` again. A static `.status-bar-fill` box
opens `<body>`, styled inline in `index.html`. In an installed iOS app it is
fixed at the top edge, full width, 12px tall and topmost. Its solid
`background-color` is the header's top colour. `background-clip: text` leaves
it unpainted, and it takes no taps.

## Rationale

Reports from other web apps on iOS 27 agree on one rule. iOS colours the
status bar from a fixed box with a solid background colour at the top edge,
and drops the blur. The box must be at least 6px tall, about 80% wide, and
within 4px of the top. FreshRSS shipped this as an invisible 11px strip,
tested on iPhone and iPad.

Our earlier strip (ADR 0548, attempt 3) had `height: var(--safe-area-top)`.
During launch `env(safe-area-inset-top)` is still 0, and iOS samples the top
edge then. So that strip was likely 0px tall when it counted. A literal height
removes that dependency. The strip is styled before the bundle loads for the
same reason.

ADR 0548's WebKit reading (no colour extension at a zero top inset) may still
hold. The reported behaviour is a separate path, and only a device test
settles it.

## Consequences

- iOS reads the style at install. An install made under 0548 keeps the opaque
  bar until it is removed and added again, and then pairs again.
- The header pads its rows by `--safe-area-top` again, so its fill runs behind
  the status bar on its own.
- `theme-color` keeps carrying the header's top colour. Android and the
  browsers' toolbars still use it.
- The gateway splash lifts the status bar tag and the strip, so a cold launch
  that opens on it meets the same top edge.
- The strip sits below a fullscreen app and the modal layer, which own the top
  edge while open. It rises over the boot splash only while the splash stands.

## Alternatives considered

- **Keep the opaque bar (ADR 0548).** It clears the blur for sure, but the
  header no longer reaches the island. The user rejected it.
- **Translucent, with the header's controls lowered below the blur.** It needs
  no strip, but the header grows by about 40pt. It is the fallback if the
  strip fails on device.
- **Translucent, and accept the blur.** Icons and badges near the top blur.
