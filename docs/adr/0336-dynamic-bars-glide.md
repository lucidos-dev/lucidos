# 0336: The mobile dynamic bars glide on a compositor transition instead of tracking each scroll event

- **Status**: Accepted (the carry and the quiet-thread reveal are superseded by 0337)
- **Date**: 2026-09-30

## Context

With *dynamic bars* on, the mobile header, the thread's title bar and its prompt
leave on a scroll down and return on a scroll up. `hooks/useHideOnScroll.ts`
moved them from JavaScript: on every scroll event it shifted each bar by that
event's delta, pixel for pixel, with no transition. The dynamic bars plan
(`docs/plans/2026-09-30-mobile-dynamic-bars.md`) settled that shape.

The user reported the motion was not smooth, the title bar included, and had
tried to fix it before without success.

The cause is structural. On iOS the scroll runs on the compositor thread. The
scroll event reaches the main thread afterwards, and it stalls outright while
the main thread is busy, which a streaming transcript keeps it. So every write
answers a scroll that has already moved on. The bars trail the content and
catch up in jumps. No per-event write can close that gap.

## Decision

The scroll only decides where the bars belong: away or shown. A CSS transition
on `translate` carries each bar there, and the compositor runs it.

- **Hysteresis.** The intent flips only after `BARS_TRAVEL_REM` of travel in one
  direction (`nextBarsIntent`), so jitter never flips it.
- **Edges.** The header and title bar stay shown within their own height of the
  top (`headerOffsetPx`). The prompt stays shown within its height of the live
  edge, unless the follow's ride carries the reader (`promptOffsetPx`).
- **Reveals.** A pane swipe, the keyboard closing, our own navigation and a
  ridden thread going quiet all bring every bar back.
- **Properties.** The motion rides `translate`. The title bar's repaint-nudge
  counter stays on `transform`, so the glide never animates it.
- **Gating.** The duration is `--bars-glide-duration`, declared only under
  `:root[data-mobile-dynamic-bars]` as `--duration-slow`. Every use falls back
  to `0s`, so pinned bars never animate. Reduced motion and the animation-speed
  slider apply through the token.

## Rationale

A transition that has started needs nothing from the main thread, so a busy
transcript cannot stall it. That is the only lever that reaches the root cause.
Everything else the hook does per event is cheap bookkeeping, and it now runs
only to change a destination, never to animate.

## Consequences

- The bars no longer follow the finger. A short drag moves nothing, and a longer
  one sends them fully away or fully back.
- Per-pane saved offsets are gone. A pane swipe reveals instead, which is what
  the user asked for.
- The keyboard still hides the header, now with a glide rather than a snap.
- A new bar joins by listing `translate var(--bars-glide-duration, 0s)` in its
  `transition` and reading its offset through `translate`. Source scans in
  `hooks/useHideOnScroll.test.ts` pin both.

## Alternatives considered

- **Keep pixel tracking, make each write cheaper.** Earlier work already moved
  the offset off `top` and off the document root. The remaining lag is the event
  arriving late, which no cheaper write fixes.
- **Track the finger during a drag, glide after release.** Closer to the old
  feel, but the drag is exactly when the main thread is busiest, so it still
  stutters under a streaming reply.
- **CSS scroll-driven animations.** They map the absolute scroll position, and
  hide-on-scroll depends on direction, which they cannot express.
