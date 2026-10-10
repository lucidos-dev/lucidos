# 0081: The transcript skeleton answers to the delay gate alone; the pre-render hold was tried twice and reverted

- **Status**: Accepted
- **Date**: 2026-08-14

## Context

A thread opening on the iOS PWA sometimes showed an empty transcript for
seconds, then all the content at once, with no skeleton. The user asked for the
skeleton to show during that wait.

The cause was real. Applying an events snapshot triggers the fold and the
markdown pass in one synchronous render. A timer cannot preempt running
JavaScript. So a snapshot landing inside `SPINNER_DELAY_MS` cancels a skeleton
that never got a frame.

The fix was a **hold**: `loadThreadEvents` raised the skeleton itself, ahead of
the gate, awaited a paint, and only then applied the rows. It shipped in the
morning and was reported broken by the evening of the same day. It was then
refined rather than removed, and reported broken twice more.

The reports were consistent, and they described the hold working as designed:
the skeleton appeared **instantly** and there was **no transition**.

## Decision

The transcript shows its loading skeleton on ONE clock, the shared
`SPINNER_DELAY_MS` delay gate, exactly like every other loader in the app.
Nothing raises it early. `shouldHoldForSkeleton`, `forcedSkeletonThreadId`,
`utils/threadRenderRate.ts` and `utils/nextPaint.ts` are deleted.

## Rationale

**The hold could not produce a good frame, whatever triggered it.** Its two
steps are the two symptoms. Raising the skeleton ahead of the gate IS showing it
instantly. Blocking the main thread with the fold immediately after IS having no
transition, because a CSS transition cannot advance while JavaScript runs. Every
open the hold fired on therefore looked like a glitch.

Tuning the trigger could only change how OFTEN the bad frame appeared, never
what it looked like. Both attempts were trigger work. The first fired on any
snapshot of 200 events or more, which is the median thread: measured against a
real workspace, 63.7% of a month's opens. The second replaced that guess with a
measured per-device render rate. It was more honest and still fired on the
reporting device, whose threads are large and whose phone is slow.

**The bug it fixed is rarer than the bug it caused.** The blank pane needs a
fast fetch AND a slow render together. On the device that reported both, the
fetch runs over a tailnet and is usually the slow half. The plain delay gate
already covers that, because its timer fires during the fetch, which is async.
So the hold spent a guaranteed glitch on most opens to cover a case the gate
mostly handled.

## Consequences

- A thread whose snapshot arrives fast and renders slowly shows a blank pane for
  the render again. That is the regression this accepts. It is bounded by the
  render window (`INITIAL_WINDOW`, 20 exchanges), which caps the markdown pass
  however large the thread is.
- The delay gate's promise holds everywhere with no exceptions: no loader
  appears unless the wait passes 300ms. One rule, and no carve-out to remember.
- ONE improvement found while chasing this stays: the overlay is keyed so it
  survives `ThreadView`'s cold-open branch switch, which is what lets its
  crossfade run at all on an iOS PWA. Everything else on this path is back to
  what it was before the hold shipped.
- An entrance fade on the overlay was tried in the same hours and reverted with
  it. Resting the skeleton transparent and ramping it up is a second gate on
  top of the delay gate. On a wait of a few hundred ms it never reaches a
  legible opacity. The user sees the bare pane and reports a blank screen with
  no loader at all. The gate decides whether a loader is owed; once it says
  yes, the loader is opaque.
- If the blank pane returns, attack the render cost. Do not put a loader in
  front of it. Windowing, chunking the fold, and moving the markdown pass off
  the main thread all address the cause.

## Alternatives considered

**Raise the size threshold instead of deleting the hold.** Rejected. It moves
the glitch to fewer opens without changing it. The tail it would still fire on
is the large threads, where the render is longest and the freeze most visible.

**Keep the measured prediction and only skip the entrance fade.** This is what
the second attempt did, and it is what made the last report worse. The forced
path deliberately showed the skeleton at full strength with no fade, since a
fade would freeze half-played under the render. That is the reported symptom
written as code.

**Show the skeleton and yield repeatedly through the fold.** Rejected as out of
scope here. It is incremental rendering, a much larger change to
`computeExchanges` and `renderExchanges`, and it is the real answer if the blank
pane returns. It was offered to the user alongside this removal and declined for
now.

**Withhold the rows until the skeleton has been visible for a minimum.** Already
rejected across the whole app (`.claude/rules/frontend.md`): holding a loader
means withholding ready content, which is what makes a slow surface feel
sluggish.
