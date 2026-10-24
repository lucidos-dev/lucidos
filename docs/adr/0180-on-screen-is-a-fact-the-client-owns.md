# 0180: On screen is a fact the client owns, not one it asks AppKit for

- **Status**: Accepted
- **Date**: 2026-09-10

## Context

A packaged window stopped answering clicks for about ten minutes and then
recovered by itself. WebKit's own log shows it suspending that page's WebContent
process, and a suspended WebContent dispatches no input. The trigger it acted on
was `isViewVisible(): window visible 0, view hidden 0, window occluded 1`.

`[NSWindow isVisible]` answered NO for a window the user was clicking.

Three things in the client made that survivable for ten minutes rather than one:

- **The heartbeat was app-global.** `crash_watchdog::heartbeat` took an
  `AppHandle` and wrote one slot, so any healthy window's page spoke for every
  other. Two windows open and the watchdog could not see either fail.
- **The watchdog watched `main` alone.** A `window-<n>` was never covered, and
  ADR 0141 parks by hiding, so a multi-window client is the ordinary case.
- **It had no notion of the screen.** Silence was a crash, and the answer was a
  reload. A window parked in the tray is silent by design. So a parked client
  reloaded its pages all night, throwing away the state ADR 0141 keeps.

## Decision

The client records whether it put each window on screen, and the watchdog reads
that instead of asking AppKit (`crates/lucidos-app/src/window_screen.rs`).
Heartbeats are per webview label, and the watchdog walks every app window.

Three inputs decide it. Our own intent covers the hides the client performs.
`NSApplicationDidHide` covers Cmd-H, which AppKit does behind our back.
`isMiniaturized` covers the yellow button. `isVisible` is deliberately not one
of them.

The same change makes the traffic-light bar a property of the SURFACE. A surface
declares its band with `data-titlebar-band`, and a reported height belongs to the
window that reported it.

## Rationale

**A watchdog that asks AppKit the same question gets the same wrong answer.**
That is the whole point. Gate the recovery on `isVisible` and it stands down in
the very case it was rebuilt for. The log says `isVisible` is the value that
lied. Intent is the one signal the suspected fault cannot corrupt.

**Intent alone is not enough, so it is not used alone.** Cmd-H and the yellow
button take a window off screen without the client asking. Left unobserved, both
turn into a reload loop against a page that is correctly quiet. Each gets its own
input rather than a fudge factor.

**The reload is the guarantee, and it is proven.** Loading a page resumes a
suspended WebContent, so the recovery works whatever stopped the page. What
changed is the detection: an on-screen page that goes quiet is now answered
inside about a minute.

**The bar belongs to the surface, not the device.** The workspace shell renders
at the user's UI scale, and the picker at the browser default. One shared height
let each move the other's lights. The picker mounts no app shell, so a
measurement keyed on `.app-header` found nothing there: it pushed nothing and
wore whichever bar the last app page had persisted.

**The AppKit trigger itself stays unproven, and nothing is fixed on a guess.**
A standalone AppKit and WKWebView probe, checked in behind the `window-probe`
feature, drove the client's own paths with wry's view hierarchy: placed while
hidden then shown, parked, reopened, miniaturized, and Cmd-H, with 45s off-screen
dwells. Neither fault reproduced. WebKit tracked visibility correctly every time
and never suspended the page, and the light placement never reverted.

## Consequences

- **A frozen window recovers in about a minute** instead of surviving until the
  user happens to interact with it.
- **A parked client is left alone.** Its pages keep their state, which is what
  makes the tray reopen instant.
- **Every app window is watched**, and one window's health no longer speaks for
  another's silence.
- **The picker's lights centre on a band it owns.** Its band is `3rem` off its
  own root font size, read from the same token the shell's bar is built from.
- **The persisted bar height is now a SEED**, not the live value. It covers the
  frames between a window appearing and its own page measuring, and the last
  surface to push wins it.
- **The probe is checked in and run by hand.** WKWebView exposes no WebDriver and
  `tauri-driver` is Linux and Windows only (ADR 0016), so this cannot be an
  automated test. It is behind a feature, so no shipped build carries it.
- **Four transitions were left untested**, for want of an API to script them: a
  Space switch, a move to a second display, a backing-scale change, and a
  fullscreen round trip. Two of them are scripted now, in the probe's `screens`
  mode: `setFrameOrigin` into another screen's visible frame is the API, and on
  two displays of different scale factors it drives both at once. Neither
  reverts the placement, and nor does a move on one display or a restated
  `setContentSize`. A Space switch and a fullscreen round trip still need the
  `watch` mode.
- **One in-tree claim is contradicted.** `app_window.rs` and
  `utils/nativeWindow.ts` say the embedded WKWebView cannot observe `orderOut:`.
  A bare WKWebView observes it immediately and every time, so whatever produced
  that observation is not generic WebKit behaviour. Neither file is corrected:
  the claim they turn on is about `hasFocus()`, which this change does not touch.

## Alternatives considered

**Gate the watchdog on `[NSWindow isVisible]`.** Three lines instead of a module,
and correct for every park the probe exercised. Rejected: it is the value the
report shows answering wrongly, so the fix would not have fixed the incident.

**Re-assert view visibility into WebKit on every show.** Toggle the `WKWebView`
hidden flag off and on through `Webview::with_webview`, so WebKit re-derives the
page's activity state from a fresh read. Rejected because the probe found nothing
needing it. It would have shipped an AppKit surface, a temporary-measure row and
a flicker risk, all against a mechanism no measurement supports.

**Re-place the lights on order-in, deminiaturize, becoming key, and the
activation-policy flip.** The obvious answer to "the placement reverted". The
probe held the placement across every one of them, including the
hidden-then-shown order `main` actually takes. Guarding a transition that does
not break it is dead code.

**Give the picker AppKit's default placement instead of a band.** Less work.
Rejected twice over. It leaves the inheritance alive for every other surface that
reports nothing. It also makes the chrome jump when one window navigates from the
picker into a workspace.

**Fold this into `native-window-active`.** One flag for both questions. Rejected,
and the two must stay apart. Notification presence asks whether the user is
LOOKING at the window. A visible but unfocused one is deliberately not active,
and gets an OS banner. Process throttling asks whether the page must keep
running, and an unfocused on-screen window must never be suspended.
