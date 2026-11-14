# 0610: The iOS shell never takes one viewport reading as settled, since WebKit can report a wrong one

- **Status**: Accepted
- **Date**: 2026-10-31

## Context

The phone's client log caught two faults in one session of the installed iOS
app.

1. **A wrong layout viewport at a wake.** The app went to the background in
   landscape (`innerHeight` 402) and came back in portrait. The first reading
   at the wake gave the portrait width with the landscape height: 402 by 402,
   and a visual viewport of 0. A window `resize` brought `innerHeight` back to
   874 a moment later. No visual-viewport resize followed, and the shell owner
   listened for nothing else, so `--app-height` stayed at 402px. That drew the
   app in the top 46% of the screen.
2. **A keyboard the visual viewport does not report.** With the composer
   focused and the keys on screen, iOS reported `visualViewport.height` equal
   to `innerHeight` (874). The shell kept its full height and the composer sat
   behind the keys. The screen above them was blank. A reload kept this state,
   and killing and relaunching the app cleared it. After the relaunch the same
   focus reported 478.

The second fault survives a full JavaScript reload. So the wrong value lives
in WebKit, not in our state. A freshly loaded page reads it on its first
measurement.

## Decision

The shell owner of `--app-height` re-reads on every signal WebKit gives:
visual-viewport `resize`, window `resize`, `orientationchange`,
`visibilitychange` and `pageshow`. No reading counts as settled past the next
signal. The owner never relies on one event type to deliver a correction.

The page does not try to reset WebKit's own viewport state. A reload already
runs everything a page can do, and it did not clear the second fault.

## Rationale

The browser's own numbers are the only authority for the viewport. A guess
from `screen` or a fixed inset fails for some layout the product ships to. So
the fix is to hear every correction the browser announces, rather than to
second-guess a reading.

The first fault is ours: iOS announced the correction, on an event the owner
ignored. The second is not ours to repair from the page. Nothing in the page
can see the keys when WebKit leaves them out of `visualViewport`. Ghost focus
and a hardware keyboard look exactly the same from JavaScript.

## Consequences

- A wake into the wrong orientation now heals on the window `resize` that
  follows it.
- The second fault stays open. The user's way out is a relaunch.
- The safe-area diagnostic also logs the written `--app-height` and WebKit's
  CSS viewport units. A report can then tell a wrong write from a wrong
  reading, and see whether CSS agrees with `innerHeight`.
- A regression test must model a wrong FIRST reading, not only a stale one
  left behind by our own state. A reload does not reset WebKit.

## Alternatives considered

- **Clamp to `screen.height` minus the safe-area insets.** True only for a
  full-screen phone window. An iPad in Split View or Stage Manager draws a
  window smaller than its screen. iOS also quotes `screen` in portrait whatever
  the orientation, so the clamp needs its own orientation guess.
- **Toggle the viewport meta tag, or nudge the scroll, to force a re-read.** A
  reload parses the meta tag and lays the page out again, and it left the
  second fault in place. The shell already nudges the scroll on each keyboard
  close (ADR 0228). Neither can be tested off a real phone, and a meta change
  can zoom the page.
- **Size the shell with CSS alone (`100dvh`).** `dvh` does not shrink for the
  iOS keyboard, which is why `--app-height` exists. The keyboard-close
  relayout and the band also read the px value the owner writes.
- **Shrink the shell by the remembered keyboard height when a focus brings no
  shrink.** The same state is ghost focus or a hardware keyboard, and the shell
  would cut the screen for those.
- **A timer that re-reads the viewport.** The always-on build polls nothing
  (`.claude/rules/frontend.md`). The perf-gated poll stays a diagnostic.
