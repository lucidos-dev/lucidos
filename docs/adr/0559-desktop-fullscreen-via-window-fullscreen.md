# 0559: The desktop client fullscreens the window, not the element; supersedes ADR 0054's frontend half

- **Status**: Accepted
- **Date**: 2026-10-23

## Context

ADR 0054 made the app view's Fullscreen real in the packaged client by enabling
WebKit element fullscreen. It named native *window* fullscreen as the escalation
if that proved wrong. It did prove wrong, in two ways:

1. **Element fullscreen needs a user gesture.** The header button is one. The
   `Ctrl+Shift+F` shortcut pressed while focus is inside the app iframe is not:
   the SDK forwards it with `postMessage`, and a message handler carries no
   activation. WebKit refuses and the CSS fallback fills only the window. A
   successful entry also focuses the frame, so the toggle after it is the one
   that fails. Users saw fullscreen that worked "sometimes".
2. **Element fullscreen moves the web view into a separate AppKit window.** It
   shows a snapshot and a placeholder while the shell re-lays out. That is the
   header flicker, also in the `.dmg`.

## Decision

**In the desktop client, the app view's Fullscreen turns on pseudo-fullscreen and
puts the calling window into native fullscreen.** A new app command,
`set_window_fullscreen(fullscreen) -> bool`, does the second half and returns
whether the window was already fullscreen. Browsers keep element fullscreen with
the CSS fallback, unchanged.

One subscriber on the pseudo-fullscreen signal drives the window, so the toggle,
Escape and navigating away all leave the window the same way. The page leaves
only a fullscreen it entered. The shell emits `window-fullscreen-changed` from
the `Resized` arm, on a change only, so a macOS-side exit (green button,
`Ctrl+Cmd+F`, a swipe) drops pseudo-fullscreen.

## Rationale

Tauri's `set_fullscreen` needs no gesture, so every entry path behaves the same.
AppKit's own space transition replaces the web view's window swap. The reported
event closes the gap ADR 0054 listed against this route: the exits Tauri did not
report. Reading `is_fullscreen()` on a resize needs no poll.

## Consequences

What we keep:

- The `macos-private-api` feature and ADR 0054's private-key risk. An app's own
  `requestFullscreen` (the video player widget) still needs WebKit element
  fullscreen.
- Host modals and toasts over a fullscreen app. Pseudo-fullscreen stacks below
  them at `--z-app-fullscreen`.

What we give up:

- The desktop client's Fullscreen is no longer the same mechanism as the
  browser's.
- A window restored fullscreen by the window-state plugin after a quit stays an
  ordinary fullscreen window. The page does not re-enter pseudo-fullscreen.
- A page reload while pseudo-fullscreen is on leaves the window fullscreen with
  the normal layout. The user leaves it with the green button.
- No test drives WKWebView (ADR 0016). Verification is manual: the button, the
  shortcut from inside an app, Escape and the green button.

## Alternatives considered

**Keep element fullscreen and request it from the forwarded shortcut.** Rejected:
no page code can give a `postMessage` handler a user activation.

**Keep element fullscreen and hide the flicker.** Rejected: the flicker is the
web view moving between AppKit windows, which the page cannot control.
