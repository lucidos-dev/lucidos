# 0193: A restored window frame no attached display can hold is corruption, and the clamp runs where it can see it

- **Status**: Accepted
- **Date**: 2026-09-16

## Context

A tester on the packaged DMG updated the client. The UI came up clipped on the
right, with content off the right edge unreachable. One restart did not clear
it. A second restart did. The maintainer could not reproduce it.

`main`'s frame is restored by `tauri-plugin-window-state`, which stores PHYSICAL
pixels. ADR 0173 left that residual deliberately, and named its cost: "a
first-launch `main` at the wrong size". tao's physical pixels are a per-display
projection of the point space. Restore them against a scale factor the capture
never used, and the window comes up at twice or half the size it was left at.

A window twice as wide as its display is exactly "content off the right edge,
unreachable". Two things let that reach the screen and stay there.

**`sanitize` had a size floor and no ceiling.** A frame wider than the work area
kept its size and got its leading edge aligned, which
`a_window_larger_than_the_work_area_aligns_with_its_leading_edge` pinned as
intended. The reachability test only asks for 120 points of title bar, so a
doubled window passed it untouched.

**The clamp could not see the frame anyway.** `clamp_restored_geometry` ran in
`setup`, which is before `app.run()`. tao defers both setters to the main
dispatch queue, and that queue is not drained until the run loop turns. So the
clamp read the geometry the window was BORN at, which is the healthy declared
default. It was a no-op on `main`'s path on every launch.

Nothing else in the client corrects a restored frame at runtime, which is why
restarting re-applied the same frame.

## Decision

**A frame no attached SCREEN can hold is corruption, of the same class as a
degenerate one.** `sanitize` caps it to the work area it lands on, and places it
there in the same step.

**The ceiling is measured against the screen, and the cap against the work
area.** The Dock is not a bound on a resize, so a window can legitimately be
taller than the work area. `Panel` carries both rects for exactly this.

**A frame that still fits SOME attached display is a preference.** It is left
exactly as it is.

**The clamp runs just before a window reaches the screen, never from `setup`.**
That is `show_startup_window`, `front_window`, `reopen_client` and
`open_app_window`. Both racers that end the deferred show arrive after the run
loop's first turn. That turn is the first moment the clamp can read what the
window will actually wear. The clamp and the show go out in one main-thread
block, in that order, and `STARTUP_SHOW_STEPS` is the value that fixes it.

## Rationale

**macOS bounds a resize to the screen, so the user cannot have dragged it.**
That is what makes the shape corruption rather than a preference. It is the same
argument the size floor already rests on: a shape no gesture can produce came
from somewhere else.

**The lenient rule keeps the case a user would notice losing.** A window sized
for a monitor that is away today comes back whole when it returns. The stricter
rule, correcting whenever the frame does not fit the display it is placed on,
catches more and costs that. The maintainer weighed the pair and chose lenient.

**Capping alone would not have helped.** A shrunk window still hangs off the
same edge, and its grab band is still reachable, so the position pass would pass
it through. Cap and place are one step, against one display, or the correction
is invisible.

**The floor wins over the ceiling.** A work area under the declared minimum
cannot be served. Capping to it would return a window the config calls unusable,
and the floor would bounce that back to the default on the next run. Re-applying
the floor after the cap is what keeps `sanitize` idempotent.

**The show is the first honest read, and it is deterministic rather than a
guess.** The frontend cannot signal ready before its document has run, and the
fallback timer is seconds out. Both are strictly later than the turn that drains
the setters. ADR 0178 rejected "anything we scheduled would be a guess about
timing", and this is not one: it is an event that cannot precede the drain.

**The order has to be a value.** Clamping after the show corrects the window in
front of the user. Two straight-line calls read the same either way, which is
the argument `app_window::placement_steps` already makes.

## Consequences

- `a_window_larger_than_the_work_area_aligns_with_its_leading_edge` is gone. Its
  replacement, `a_window_larger_than_the_screen_is_capped_to_its_work_area`,
  states the new rule and says what the old one cost.
- A launch that comes up menu-bar-only never reaches `show_startup_window`. So
  `reopen_client` clamps whether or not it placed a frame, and `front_window`
  clamps a window that is not up yet. That second one is the path a notification
  tap takes, and it was the hole the first fix left. `front_window` also focuses
  a window already on screen, and there it must NOT clamp: moving one the user
  parked is what ADR 0173 refused.
- The order is ISSUED, not applied. tao defers a placement to the main dispatch
  queue and runs an order-front inline. So a launch the clamp corrects can paint
  one frame at the restored size before the correction lands. Issuing the
  correction first is what bounds that to one frame. Deferring the show a second
  hop to close it would be a guess about queue ordering, which ADR 0178 refuses.
- `docs/known-gaps.md` § "The restore clamp judges the geometry a window had
  before the placement" loses its `setup` half. The reopen still reads early.
- `.window-state.json` stays physical. ADR 0173 § Alternatives rejected taking
  `SIZE` and `POSITION` off the plugin, and that stands: it is the only record
  of `main`'s frame while `main` sits on the picker.
- The lenient rule leaves one shape uncorrected: a frame that fits a large
  attached display while the window sits on a small one. The user can drag it
  over, and the big display is right there.
- None of this is verifiable outside a packaged build (ADR 0016). `sanitize` is
  pure and fully covered; the seam the clamp now runs from is inspection plus
  `STARTUP_SHOW_STEPS`.

## Alternatives considered

**Leave `sanitize` alone and fix the units instead.** Converting the plugin's
record to points is the root fix, and ADR 0173 already weighed it. It loses
`main`'s frame whenever `main` is on the picker, which the session record cannot
key. A clamp that catches the result is cheaper, and it catches every other
source of a bad frame too.

**Correct whenever the frame does not fit the display it is placed on.** The
stricter rule. It catches a window dragged onto a smaller second display. It
also shrinks a window whose big monitor is merely away, which the user would
have to undo by hand every time.

**Defer the clamp by a timer after `setup`.** A guess about how long the drain
takes, and ADR 0178 rejected that class already. The show is an event, not a
duration.

**Clamp on the first `Resized` instead.** It fires when the placement lands,
which is the right moment. It also fires on every user drag afterwards, so the
clamp would stop being a one-shot and become a resize policy. That is a
different decision, and a larger one: it would move windows people put where
they wanted them.

**Keep the size and nudge harder.** ADR 0173 § Alternatives already refused to
widen the clamp for a window hanging off an edge, because a user may park one
there deliberately. That reasoning holds for a window that FITS. It does not
reach a window no display can hold, which no gesture can produce.
