import { describe, it, expect } from 'vitest';

import {
  computeScrollIndicator,
  counterScaledRadiusPx,
  estimateUnrenderedHeightPx,
  nextIndicatorVisibility,
  MAX_THUMB_FRACTION,
  MIN_THUMB_PX,
  type ScrollEventContext,
  type ScrollIndicatorInput,
} from '../scrollIndicator';

/** A whole thread in the DOM (no windowing), 10 screens tall, at the very top. */
function base(over: Partial<ScrollIndicatorInput> = {}): ScrollIndicatorInput {
  return {
    scrollTop: 0,
    scrollHeight: 8000,
    clientHeight: 800,
    renderFromIndex: 0,
    totalExchanges: 40,
    trackHeightPx: 600,
    ...over,
  };
}

describe('computeScrollIndicator: position maps the visible content region', () => {
  it('sits flush with the top of the track at scrollTop 0', () => {
    expect(computeScrollIndicator(base()).thumbOffsetPx).toBe(0);
  });

  it('sits flush with the bottom of the track at the maximum scroll', () => {
    const input = base({ scrollTop: 7200 }); // scrollHeight - clientHeight
    const { thumbHeightPx, thumbOffsetPx } = computeScrollIndicator(input);
    expect(thumbOffsetPx + thumbHeightPx).toBeCloseTo(input.trackHeightPx, 5);
  });

  it('lands mid-track at the halfway scroll position', () => {
    const input = base({ scrollTop: 3600 });
    const { thumbHeightPx, thumbOffsetPx } = computeScrollIndicator(input);
    expect(thumbOffsetPx).toBeCloseTo((input.trackHeightPx - thumbHeightPx) / 2, 5);
  });

  it('advances monotonically as the user scrolls down', () => {
    const at = (scrollTop: number) => computeScrollIndicator(base({ scrollTop })).thumbOffsetPx;
    expect(at(0)).toBeLessThan(at(1800));
    expect(at(1800)).toBeLessThan(at(3600));
    expect(at(3600)).toBeLessThan(at(7200));
  });

  it('sizes the thumb as the viewport share of the scrollable content', () => {
    // 800 of 8000 is a tenth of the content, so a tenth of a 600px track.
    expect(computeScrollIndicator(base()).thumbHeightPx).toBeCloseTo(60, 5);
  });
});

describe('computeScrollIndicator: the un-rendered head of a windowed thread', () => {
  // The transcript renders a trailing slice only (threadWindow.ts). The scroller
  // therefore describes the rendered tail, which is what put the native thumb
  // near the top of its track while the content on screen was deep in the thread.
  const windowed = base({ renderFromIndex: 180, totalExchanges: 200, scrollTop: 0 });

  it('does not report the top of the thread when only the tail is rendered', () => {
    const geo = computeScrollIndicator(windowed);
    expect(geo.thumbOffsetPx).toBeGreaterThan(0);
    // 20 of 200 exchanges rendered: the viewport is somewhere in the last tenth.
    expect(geo.thumbOffsetPx).toBeGreaterThan(windowed.trackHeightPx * 0.5);
  });

  it('reports further down the track than the same scroll position with everything rendered', () => {
    const whole = computeScrollIndicator(base({ ...windowed, renderFromIndex: 0 }));
    const partial = computeScrollIndicator(windowed);
    expect(partial.thumbOffsetPx).toBeGreaterThan(whole.thumbOffsetPx);
  });

  it('draws a smaller thumb, because the thread is larger than the rendered slice', () => {
    const whole = computeScrollIndicator(base({ ...windowed, renderFromIndex: 0 }));
    const partial = computeScrollIndicator(windowed);
    expect(partial.thumbHeightPx).toBeLessThan(whole.thumbHeightPx);
  });

  it('settles onto the exact position once the window grows to cover the thread', () => {
    // Same scroller metrics, window expanded from 20 to all 200: the estimate
    // collapses to 0 and the mapping becomes the plain one.
    const expanded = computeScrollIndicator({ ...windowed, renderFromIndex: 0 });
    expect(expanded.thumbOffsetPx).toBe(0);
  });

  it('still lands flush with the bottom of the track at the end of a windowed thread', () => {
    const input = { ...windowed, scrollTop: 7200 };
    const { thumbHeightPx, thumbOffsetPx } = computeScrollIndicator(input);
    expect(thumbOffsetPx + thumbHeightPx).toBeCloseTo(input.trackHeightPx, 5);
  });
});

describe('estimateUnrenderedHeightPx', () => {
  it('is zero when the whole thread is rendered', () => {
    expect(estimateUnrenderedHeightPx(base())).toBe(0);
  });

  it('scales with how many exchanges are missing', () => {
    const few = estimateUnrenderedHeightPx(base({ renderFromIndex: 10, totalExchanges: 200 }));
    const many = estimateUnrenderedHeightPx(base({ renderFromIndex: 100, totalExchanges: 200 }));
    expect(many).toBeGreaterThan(few);
  });

  it('uses the rendered slice mean: 20 of 200 rendered over 8000px estimates 72000px above', () => {
    expect(estimateUnrenderedHeightPx(base({ renderFromIndex: 180, totalExchanges: 200 })))
      .toBeCloseTo(72000, 5);
  });

  it('is zero rather than infinite when nothing is rendered to average over', () => {
    expect(estimateUnrenderedHeightPx(base({ renderFromIndex: 40, totalExchanges: 40 }))).toBe(0);
  });

  it('ignores a renderFromIndex past the total (a stale window against a shrunken thread)', () => {
    expect(estimateUnrenderedHeightPx(base({ renderFromIndex: 500, totalExchanges: 40 }))).toBe(0);
  });
});

describe('computeScrollIndicator: when to draw nothing', () => {
  it('hides when the content fits the viewport', () => {
    expect(computeScrollIndicator(base({ scrollHeight: 800 })).visible).toBe(false);
  });

  it('hides when the content is shorter than the viewport', () => {
    expect(computeScrollIndicator(base({ scrollHeight: 400 })).visible).toBe(false);
  });

  it('hides before layout has given the transcript a height', () => {
    expect(computeScrollIndicator(base({ clientHeight: 0 })).visible).toBe(false);
  });

  it('hides before layout has given the track a height', () => {
    expect(computeScrollIndicator(base({ trackHeightPx: 0 })).visible).toBe(false);
  });
});

describe('computeScrollIndicator: iOS elastic bounce', () => {
  // WebKit reports scrollTop below 0 and past the maximum while the scroller is
  // rubber-banding. Untreated that pushes the thumb out of both ends of its track.
  it('pins to the top of the track while bouncing past the top', () => {
    expect(computeScrollIndicator(base({ scrollTop: -220 })).thumbOffsetPx).toBe(0);
  });

  it('pins to the bottom of the track while bouncing past the bottom', () => {
    const input = base({ scrollTop: 9000 });
    const { thumbHeightPx, thumbOffsetPx } = computeScrollIndicator(input);
    expect(thumbOffsetPx + thumbHeightPx).toBeCloseTo(input.trackHeightPx, 5);
  });
});

describe('nextIndicatorVisibility: what keeps the indicator up', () => {
  const ev = (over: Partial<ScrollEventContext> = {}): ScrollEventContext => ({
    userScrolling: false,
    programmaticScroll: false,
    repaintNudge: false,
    ...over,
  });

  it('a touch drag summons it and starts the fade countdown', () => {
    expect(nextIndicatorVisibility(false, ev({ userScrolling: true })))
      .toEqual({ shown: true, armHideTimer: true });
  });

  it('stays lit through momentum that outlives the touch window', () => {
    // THE REGRESSION. iOS momentum on a long transcript routinely runs longer
    // than USER_SCROLL_WINDOW_MS (1200ms), so scroll events keep arriving with
    // `userScrolling` already false. The first version only restarted the
    // countdown while that flag was true, so the indicator faded out with the
    // content still visibly moving.
    expect(nextIndicatorVisibility(true, ev({ userScrolling: false })))
      .toEqual({ shown: true, armHideTimer: true });
  });

  it('lets the countdown expire once a streaming auto-tail takes over the scroller', () => {
    // A programmatic go-to-bottom is not the user scrolling. Without this, a
    // thread that started streaming after the user's flick would hold the
    // indicator on screen for as long as tokens kept arriving.
    expect(nextIndicatorVisibility(true, ev({ programmaticScroll: true })))
      .toEqual({ shown: true, armHideTimer: false });
  });

  it('is never summoned by a streaming auto-tail alone', () => {
    expect(nextIndicatorVisibility(false, ev({ programmaticScroll: true })))
      .toEqual({ shown: false, armHideTimer: false });
  });

  it('ignores the iOS compositor-recovery nudge entirely', () => {
    // The nudge writes +/-1px on a ~200ms throttle and undoes it a frame later.
    // Treated as motion it would hold a once-summoned indicator up forever on a
    // streaming thread, and summon nothing but confusion on an idle one.
    expect(nextIndicatorVisibility(false, ev({ repaintNudge: true })))
      .toEqual({ shown: false, armHideTimer: false });
    expect(nextIndicatorVisibility(true, ev({ repaintNudge: true })))
      .toEqual({ shown: true, armHideTimer: false });
  });

  it('still honours a real drag that races a nudge', () => {
    // Same dual gate as useHideOnScroll: a nudge is only ignorable while the
    // user is NOT dragging, or it would eat the user's own scroll events.
    expect(nextIndicatorVisibility(false, ev({ repaintNudge: true, userScrolling: true })))
      .toEqual({ shown: true, armHideTimer: true });
  });

  it('stays down until something summons it', () => {
    expect(nextIndicatorVisibility(false, ev())).toEqual({ shown: false, armHideTimer: false });
  });

  it('arms the countdown for a drag that overlaps one of our own navigations', () => {
    // Otherwise the indicator is summoned with nothing to turn it off. A finger
    // landing on the transcript mid-glide (a chevron tap the reader interrupts,
    // a deep-link still settling) has both flags genuinely true together, and if
    // the scroller then goes quiet the indicator stays lit for good.
    expect(nextIndicatorVisibility(false, ev({ userScrolling: true, programmaticScroll: true })))
      .toEqual({ shown: true, armHideTimer: true });
  });

  it('arms the countdown for a drag that races a nudge during streaming', () => {
    // The three-flag corner: the user wins over both exclusions at once.
    expect(nextIndicatorVisibility(false, ev({
      userScrolling: true, programmaticScroll: true, repaintNudge: true,
    }))).toEqual({ shown: true, armHideTimer: true });
  });

  it('never turns the indicator on without starting the countdown that turns it off', () => {
    // The countdown is the ONLY thing that clears `shown`, so a summon that does
    // not arm one is a permanently stuck indicator. Exhaustive over the input
    // space rather than case-by-case: the stuck states were both corners nobody
    // thought to enumerate.
    for (const wasShown of [false, true]) {
      for (const userScrolling of [false, true]) {
        for (const programmaticScroll of [false, true]) {
          for (const repaintNudge of [false, true]) {
            const label = JSON.stringify({ wasShown, userScrolling, programmaticScroll, repaintNudge });
            const out = nextIndicatorVisibility(wasShown, {
              userScrolling, programmaticScroll, repaintNudge,
            });
            if (out.shown && !wasShown) {
              expect(out.armHideTimer, `summoned without a countdown: ${label}`).toBe(true);
              // And a summon can only ever come from the user.
              expect(userScrolling, `summoned by something other than a drag: ${label}`).toBe(true);
            }
            // The countdown is pointless when there is nothing lit to hide.
            if (!out.shown) {
              expect(out.armHideTimer, `countdown armed while dark: ${label}`).toBe(false);
            }
          }
        }
      }
    }
  });
});

describe('counterScaledRadiusPx: the caps stay round at every thumb length', () => {
  // A 0.25rem bar at the 18px mobile root, so half its width is 2.25px. That is
  // the horizontal radius, and a semicircular cap needs the vertical one to
  // render equal to it.
  const HALF_WIDTH = 2.25;

  it('renders a vertical radius equal to the horizontal one, whatever the scale', () => {
    for (const scaleY of [0.02, 0.1, 0.5, 0.89, 1, 2, 7.5, 18]) {
      const rendered = counterScaledRadiusPx(HALF_WIDTH, scaleY) * scaleY;
      expect(rendered, `scaleY=${scaleY}`).toBeCloseTo(HALF_WIDTH, 6);
    }
  });

  it('is a no-op at scale 1', () => {
    expect(counterScaledRadiusPx(HALF_WIDTH, 1)).toBe(HALF_WIDTH);
  });

  it('shrinks the specified radius when the thumb is scaled up', () => {
    // Scaling up is what stretched the caps into points.
    expect(counterScaledRadiusPx(HALF_WIDTH, 4)).toBeCloseTo(HALF_WIDTH / 4, 6);
  });

  it('grows the specified radius when the thumb is scaled down', () => {
    expect(counterScaledRadiusPx(HALF_WIDTH, 0.25)).toBeCloseTo(HALF_WIDTH * 4, 6);
  });

  it('falls back to the undistorted radius for a degenerate scale', () => {
    for (const bad of [0, -1, NaN, Infinity]) {
      expect(counterScaledRadiusPx(HALF_WIDTH, bad), `scaleY=${bad}`).toBe(HALF_WIDTH);
    }
  });
});

describe('computeScrollIndicator: thumb size bounds', () => {
  it('never shrinks below the readable floor on a very long thread', () => {
    const geo = computeScrollIndicator(base({ scrollHeight: 400000 }));
    expect(geo.thumbHeightPx).toBe(MIN_THUMB_PX);
  });

  it('keeps a floored thumb inside the track at the bottom of a very long thread', () => {
    const input = base({ scrollHeight: 400000, scrollTop: 399200 });
    const { thumbHeightPx, thumbOffsetPx } = computeScrollIndicator(input);
    expect(thumbOffsetPx + thumbHeightPx).toBeCloseTo(input.trackHeightPx, 5);
  });

  it('never grows past the track when the content barely overflows', () => {
    const input = base({ scrollHeight: 810, clientHeight: 800, trackHeightPx: 600 });
    const geo = computeScrollIndicator(input);
    expect(geo.thumbHeightPx).toBeLessThanOrEqual(input.trackHeightPx);
    expect(geo.thumbOffsetPx).toBeGreaterThanOrEqual(0);
  });

  // Derived from the constant, never hardcoded: the cap is a tuning knob, and a
  // test that restates its value only reports that someone turned it.
  const CAPPED_PX = 600 * MAX_THUMB_FRACTION;
  /** Content height at which the proportional answer IS the cap. */
  const CROSSOVER = 800 / MAX_THUMB_FRACTION;

  it('is tuned so the cap is the binding limit on this fixture', () => {
    // Every case below expects the CAP to decide the height. Turn the knob far
    // enough down and the FLOOR would decide instead, and each of them would
    // fail with a confusing off-by-a-lot rather than saying why.
    expect(CAPPED_PX).toBeGreaterThan(MIN_THUMB_PX);
  });

  it('stops a barely-overflowing thread filling the track', () => {
    // Proportionally this is 800/810 of the track: a 592px slab on a 600px
    // track, with 8px of travel to show a position in.
    const input = base({ scrollHeight: 810, clientHeight: 800, trackHeightPx: 600 });
    expect(computeScrollIndicator(input).thumbHeightPx).toBeCloseTo(CAPPED_PX, 5);
  });

  it('caps every thread shorter than the crossover at the same height', () => {
    // Below the crossover the proportional answer is above the cap, so they all
    // land on it. Above it, proportion takes over again (the next test).
    for (const scrollHeight of [810, CROSSOVER * 0.5, CROSSOVER * 0.9, CROSSOVER - 1]) {
      const geo = computeScrollIndicator(base({ scrollHeight, clientHeight: 800, trackHeightPx: 600 }));
      expect(geo.thumbHeightPx, `scrollHeight=${scrollHeight}`).toBeCloseTo(CAPPED_PX, 5);
    }
  });

  it('goes back to being proportional once the thread passes the cap', () => {
    // At the crossover exactly, the proportional answer and the cap agree.
    expect(computeScrollIndicator(base({ scrollHeight: CROSSOVER, clientHeight: 800, trackHeightPx: 600 })).thumbHeightPx)
      .toBeCloseTo(CAPPED_PX, 5);
    // Twice the crossover is half the cap, and untouched by it.
    expect(computeScrollIndicator(base({ scrollHeight: CROSSOVER * 2, clientHeight: 800, trackHeightPx: 600 })).thumbHeightPx)
      .toBeCloseTo(CAPPED_PX / 2, 5);
  });

  it('still lands flush with both ends of the track when capped', () => {
    // The cap changes the thumb's size, so it also changes its travel. Both
    // ends must still be reachable or a short thread would look stuck.
    const input = base({ scrollHeight: 810, clientHeight: 800, trackHeightPx: 600 });
    expect(computeScrollIndicator({ ...input, scrollTop: 0 }).thumbOffsetPx).toBe(0);
    const atEnd = computeScrollIndicator({ ...input, scrollTop: 10 });
    expect(atEnd.thumbOffsetPx + atEnd.thumbHeightPx).toBeCloseTo(600, 5);
  });

  it('keeps the floor winning over the cap on a track too short for both', () => {
    // A 30px track wants a 15px ceiling but the floor is 24px. The thumb takes
    // the floor, and the track is still the hard bound.
    const geo = computeScrollIndicator(base({ scrollHeight: 810, clientHeight: 800, trackHeightPx: 30 }));
    expect(geo.thumbHeightPx).toBe(MIN_THUMB_PX);
    expect(geo.thumbOffsetPx).toBeGreaterThanOrEqual(0);
  });

  it('never exceeds the track even when the floor exceeds it', () => {
    const geo = computeScrollIndicator(base({ scrollHeight: 810, clientHeight: 800, trackHeightPx: 20 }));
    expect(geo.thumbHeightPx).toBeLessThanOrEqual(20);
    expect(geo.thumbOffsetPx).toBeGreaterThanOrEqual(0);
  });
});
