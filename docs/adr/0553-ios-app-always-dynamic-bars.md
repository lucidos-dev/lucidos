# 0553: The installed iOS app always runs dynamic bars and the strip is gone, superseding 0552

- **Status**: Accepted
- **Date**: 2026-10-23

## Context

ADR 0552 split the installed iOS app's top bar by `mobile_dynamic_bars`. With
the preference off, a 12px strip was meant to make iOS 27 paint an opaque bar.

The phone's safe-area log disproves it. Two strip-mode launches logged a 12px
strip at the top edge, a 59px top inset and a full 852px page. iOS 27.0.1 kept
the translucent bar, and the header icons sat inside the blur. A reinstall did
not change that.

## Decision

In the installed iOS app, dynamic bars are always on, whatever
`mobile_dynamic_bars` holds. The app always uses the clearance shape: a
translucent bar, the header behind the island, and its controls 24px below the
top inset. Other clients keep the preference as it was.

Strip mode goes entirely: the pre-bundle mode script, the device mirror of the
preference, the `.status-bar-fill` markup and CSS, the `data-ios-status-bar`
attribute, and the gateway splash's copies of them.

## Rationale

The strip is the only way ADR 0552 had to show an opaque bar, and iOS 27 does
not honour it. A setting that changes nothing is worse than no setting. The
user prefers dynamic bars on the phone ("they're the best at this point"), and
the clearance shape is the one that works.

## Consequences

- The Settings row shows the switch on and disabled in the installed iOS app,
  with a note that it is always on there. The relaunch note is gone.
- The stored preference and its default stay. Only the installed iOS app
  ignores it.
- The safe-area diagnostic stays, without its `mode` and `strip` fields.
- Desktop, Android, Safari tabs and the Tauri app are unchanged.

## Alternatives considered

- **Keep the strip and wait for iOS to honour it.** There is no sign it will,
  and the user sees blurred icons today.
- **Hide the preference in the iOS app.** A disabled switch with a note tells
  the user why the pinned header is not on offer.
