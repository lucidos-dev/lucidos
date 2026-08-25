# 0092: system theme follows the OS live, with the iOS snapshot-pass flips guarded out

- **Status**: Accepted
- **Date**: 2026-08-18

## Context

A `theme: system` preference used to resolve against the OS once per page load,
plus whatever the `prefers-color-scheme` change event delivered later. That
listener was skipped on iOS. Telemetry had caught WKWebView firing it with the
wrong value 24 or more times in one session, flashing the page light.

The result was a theme that went stale in two different ways. An installed iOS
PWA is resumed rather than reloaded, and had no listener. It kept its boot theme
for as long as the webview lived. Off iOS the listener was live, but a change
announced to a frozen tab or a sleeping machine was lost. Nothing re-read the
value on wake. Either way the user pressed Dark by hand every evening.

## Decision

`system` re-resolves on the media query's `change` **and** on the page's own
resume, on every platform, iOS included. Three guards decide whether a
re-resolve paints: the preference still follows the OS, the document is visible,
and the resolved value differs from what is already on `<html>`. Every trigger
schedules one shared settle timer, and the value is re-READ when that fires,
never taken from the event.

The platform carve-out is gone from both the shell
(`crates/lucidos-app/src/store/actions/preferences.ts`) and the SDK
(`packages/lucidos-sdk/src/ui.ts`).

## Rationale

The carve-out was written as though WKWebView reports garbage at random. The
real mechanism is documented and narrow, which is what makes a guard possible.

A UIKit engineer's explanation is recorded in
[rdar://7213631](https://openradar.appspot.com/7213631). An app entering the
background has its trait collection flipped to the opposite appearance and back,
so iOS can render both app-switcher snapshots. WKWebView passes each flip into
the page as a real media query change. The events were therefore never corrupt.
They described an appearance that genuinely existed, for the moments the app was
being snapshotted.

That is exactly when the user is not looking, so **visibility** is the guard
that fits the cause. The radar records the same symptom we saw, the content
"abruptly shifting from light to dark when the app is selected". The **settle**
re-read covers the residual race, since the bogus value is immediately followed
by its correction. The **difference** check keeps a wake that changed nothing
from re-tinting the title bar and re-asserting every style override.

Resume is the other half, and it is the one that answers the actual report. It
is the only moment an installed iOS PWA offers, and off iOS it repairs a change
event lost to a frozen tab.

### The residual the snapshot model does not explain

One earlier observation does not fit, and is recorded here rather than argued
away. `fix(theme): skip loadPreferences re-apply when theme value unchanged`
caught a **synchronous** read returning light on a dark device, ten seconds
into a foreground session, watched live through an in-PWA debug overlay. No
backgrounding was involved, so the trait-collection pass above cannot account
for it.

So the guards narrow this class rather than close it. A wrong value read while
the page is genuinely visible still paints, and is corrected by the next event.
Three things bound the damage. The difference check means only a *changed*
value paints at all, which is that commit's own guard, kept. Nothing acts on an
event's value, so a lie shorter than the settle delay never lands. And
`applyTheme` samples the media query once, handing that value to both the paint
and its `__themeLogEvt` breadcrumb, so a recurrence is legible in `engine.log`.

The settle delay is not a measured number. It was picked to outlast the
snapshot pass, which is fast, and 300ms is imperceptible on a theme change. If
the residual above turns out to be common, the breadcrumbs are what would say
so, and the delay is the knob.

## Consequences

- A theme flip is applied a few hundred milliseconds after the OS announces it,
  on every platform. That is the price of one code path instead of two, and it
  is imperceptible on a theme change.
- The shell and every app iframe run the same contract. An open app cannot sit
  on a stale theme while the shell around it moves.
- The two guards are load-bearing rather than defensive. Dropping the visibility
  check re-opens the flash on iOS, so it is pinned by unit tests in both
  surfaces rather than left to review.
- **iOS 17 is not fixed by this, and cannot be.** A home-screen PWA froze
  `prefers-color-scheme` at its launch value there, so no listener and no
  re-read can move it ([Apple Developer Forums thread](https://developer.apple.com/forums/thread/739154),
  FB12858610). The original reporter confirms it fixed in iOS 18, and our own
  telemetry shows change events arriving there.

## Alternatives considered

**Keep the carve-out and add only a resume re-read.** This was the first plan,
and the research above retired it. It leaves the theme frozen on iPhone for
anyone who keeps the app foregrounded through a flip. It also keeps two code
paths for a problem one guard solves.

**Poll the media query while the page is visible.** The fallback for a listener
that could not be trusted. With the guards in place the listener is trusted, so
a poll is a second mechanism doing the first one's job. It also samples at
arbitrary moments, which is the shape the carve-out existed to avoid.

**Resolve `system` in CSS, with `@media (prefers-color-scheme: …)` token
blocks.** The architecturally cleaner answer: the browser resolves it, so
staleness stops being representable. Rejected on cost and risk, not on merit.

It rewrites both token blocks, roughly eight component-level
`html[data-theme=…]` rules and the engine-served iframe stylesheet. It still
could not cover the inline pre-stylesheet paint, the `theme-color` meta tag,
`style.colorScheme` or the Tauri title-bar tint without a JS resolve anyway. So
it is a FOUC-risking refactor of the appearance boot contract, and this report
did not need one. Worth revisiting on its own terms.
