# 0065: Every transcript navigation shares one rAF easeOutCubic tween writing scrollTop directly

- **Status**: Accepted
- **Date**: 2026-08-13

## Context

The chat transcript is moved by several navigations: the up and down chevrons,
turn stepping, a deep link, and a submit's landing. Each arrived separately. The
deep link originally used the browser's native smooth `scrollIntoView` while the
chevrons ran a hand-written loop. The same jump therefore felt different
depending on what asked for it.

Two platform facts constrain the implementation. iOS silently no-ops
`scrollTo({behavior:'smooth'})` and `scrollIntoView` during viewport
transitions. Safari also lags `scrollTop` READS mid-animation, which janks
anything measuring the live position each frame.

## Decision

One tween, `animateScroll` in `components/chat/scrollState.ts`, serves every
navigation that moves the transcript. It is:

- driven by `requestAnimationFrame`,
- writing `scrollTop` directly, fractionally, never rounded,
- a time-based easeOutCubic curve over a distance-scaled, clamped duration,
- re-reading its TARGET every frame, and its start position never.

"Never rounded" scopes to this tween, not to the transcript generally. The
scroll-anchor correction is a single frameless write and rounds on purpose:
ADR 0078.

## Rationale

**rAF, not `setTimeout`.** rAF is vsync-aligned, so the motion stays smooth. A
`setTimeout(16)` loop races the refresh cycle and stutters. On iOS that read as
the scroll "dragging" or not wanting to move.

**A direct `scrollTop` write, not a native smooth scroll.** The native call is
the one iOS drops during a viewport transition. It also cannot be tuned, and
cannot reach the top of a transcript that has only just rendered in full.

**Time-based easing, not exponential smoothing.** Distance-based smoothing has
an unbounded asymptotic tail. Near the target every frame moves a tinier amount,
and the sub-pixel tail rounds to alternating 0px and 1px steps. A hard cutoff
was needed to end it, and that cutoff left a visible jump.

A fixed-duration eased tween has one continuous deceleration the curve owns
end-to-end, and it lands exactly on the target. easeOutCubic front-loads
velocity, so the first frame still takes a big step and the scroll reacts
instantly to the tap. Progress is measured in elapsed milliseconds, so it feels
identical at 60Hz and 120Hz.

**The target is re-read per frame; the start is captured once.** A moving target
is tracked rather than overshot, whether it is a streaming thread's growing
bottom or content settling above a landing. The eased fraction is applied
between the captured start and the LIVE target, so the curve still lands
cleanly. Never reading `scrollTop` back is what avoids Safari's lagging read.

**The duration scales with the initial distance, clamped.** A short hop and a
long haul share one deceleration SHAPE at different speeds, so a 400px jump and
a 20000px jump settle identically.

**Fractional writes.** On a 2x or 3x display the sub-pixel position is what
makes the slow final approach read as smooth instead of stepping whole pixels.

## Consequences

- Every jump feels identical. The deep link inherits the iOS-reliable direct
  write plus moving-target tracking for free.
- The tween is inherently bounded, since `t` reaches 1 within the duration. A
  moving target cannot make it loop.
- One tween runs at a time. Every entry point cancels the one in flight, so a
  down-tap right after an up-tap wins cleanly.
- Reduced motion is honoured by writing the target once, on every path. The
  native smooth scroll ignored the preference.
- Pace is tuned by two knobs, `SCROLL_MAX_MS` and `SCROLL_PX_PER_MS`. Lower them
  to make every navigation faster.

## Alternatives considered

**Native `scrollIntoView({behavior: 'smooth'})` for the deep link.** What it
originally used. It is a second, un-tunable, slower motion, it fixes its target
at call time, and iOS drops it during a viewport transition.

**A `setTimeout` loop.** Cheaper to write, and what the first chevron used. It
races the refresh cycle and stutters visibly on iOS.

**Exponential smoothing (`current += remaining * fraction`).** Simple and
self-terminating in principle. In practice the asymptotic tail janks on the
settle and needs a snap cutoff. That is a visible jump at the end of every
navigation.

**A yield guard, so a tween stands down for a competing scroll.** Rejected for
the chevrons, because an explicit tap must always reach its target. The
stand-downs that do exist are narrow and named at their call sites.
