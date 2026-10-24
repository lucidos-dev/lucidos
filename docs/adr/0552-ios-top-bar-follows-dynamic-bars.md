# 0552: The installed iOS app's top bar follows dynamic bars, frozen per launch: a solid strip when fixed, a 24px clearance under a translucent bar when dynamic, superseding 0551

- **Status**: Superseded by [0553](0553-ios-app-always-dynamic-bars.md)
- **Date**: 2026-10-23

## Context

ADR 0551 ran the installed iOS app under a translucent status bar with no top
strip. It lowered the header's controls by a clearance the user tuned with a
Debugging slider. The user settled it: 24pt.

They also want the shape to depend on *dynamic bars*. With dynamic bars on, the
header scrolls away, so content passing behind a clear status bar is fine. With
them off, the header never moves, and a solid bar reads as the header's own top.

## Decision

The mode follows `mobile_dynamic_bars`, read once per launch:

- **Dynamic bars on (`clearance`):** ADR 0551's look. No strip, and the header's
  controls start at the top inset plus a constant 24px.
- **Dynamic bars off (`strip`, also the default):** ADR 0549's 12px strip opens
  `<body>`. On iOS 27 it makes iOS paint an opaque bar in the header colour
  and start the page below it. There is no blur and no clearance.

A script in `index.html`, before any bundle, reads a device mirror of the
preference and writes `data-ios-status-bar` on `<html>`. The strip's CSS and
`statusBarBlurClearance.ts` read that one attribute and never the live
preference.

## Rationale

iOS samples the top edge while the app launches. So the strip must be in the
first document, and the mode must be known before the bundle loads. A device
mirror in `localStorage` is the pattern the theme and autocorrect already use.

Freezing the mode keeps the geometry consistent. A mid-session toggle would
change the strip and the clearance while the safe-area floor still holds the
old page height.

## Consequences

- The Dynamic bars row in Settings says a relaunch applies the top bar, in the
  installed iOS app only. Scroll behaviour still changes at once.
- The first launch after this update with dynamic bars on shows strip mode once,
  because the mirror is empty until the preference loads.
- The gateway splash carries the strip and the mode script, sliced verbatim from
  `index.html`. It also stamps a `<base>` from its url, so the script finds the
  workspace slug.
- The clearance slider, its storage key and its log line are gone. The safe-area
  log stays, so a fixed-mode launch can be confirmed on device.
- Desktop, Android, Safari tabs and the Tauri app are unchanged.

## Alternatives considered

- **Switch the top bar live.** The strip would change under a running page, and
  iOS only samples the top edge at launch. A relaunch is honest.
- **Always the strip, or always the clearance (ADRs 0549, 0551).** Each fits one
  of the two scroll behaviours and looks wrong in the other.
- **Decide the mode from the bundle.** The strip would arrive after iOS has
  sampled the edge, which is the failure ADR 0549 recorded.
