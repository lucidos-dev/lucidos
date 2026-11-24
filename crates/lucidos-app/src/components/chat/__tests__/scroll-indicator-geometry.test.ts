import { describe, it, expect } from 'vitest';

import {
  computeScrollIndicator,
  counterScaledRadiusPx,
  nextIndicatorVisibility,
  MAX_THUMB_FRACTION,
  MIN_THUMB_PX,
  type ScrollEventContext,
  type ScrollIndicatorInput,
} from '../scrollIndicator';

function base(over: Partial<ScrollIndicatorInput> = {}): ScrollIndicatorInput {
  return {
    scrollTop: 0,
    scrollHeight: 8000,
    clientHeight: 800,
    trackHeightPx: 600,
    ...over,
  };
}

describe('computeScrollIndicator: position maps the visible content region', () => {
  it('sits flush with the top of the track at scrollTop 0', () => {
    expect(computeScrollIndicator(base()).thumbOffsetPx).toBe(0);
  });

  it('sits flush with the bottom of the track at the maximum scroll', () => {
    const input = base({ scrollTop: 7200 });
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
    expect(computeScrollIndicator(base()).thumbHeightPx).toBeCloseTo(60, 5);
  });
});

describe('computeScrollIndicator: it measures the drawn slice (ADR 0258)', () => {
  it('shrinks and moves down when older turns are drawn above the reader', () => {
    const before = computeScrollIndicator(base({ scrollTop: 400 }));
    const after = computeScrollIndicator(base({ scrollTop: 400 + 4000, scrollHeight: 8000 + 4000 }));
    expect(after.thumbHeightPx).toBeLessThan(before.thumbHeightPx);
    expect(after.thumbOffsetPx).toBeGreaterThan(before.thumbOffsetPx);
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
    expect(nextIndicatorVisibility(true, ev({ userScrolling: false })))
      .toEqual({ shown: true, armHideTimer: true });
  });

  it('lets the countdown expire once a streaming auto-tail takes over the scroller', () => {
    expect(nextIndicatorVisibility(true, ev({ programmaticScroll: true })))
      .toEqual({ shown: true, armHideTimer: false });
  });

  it('is never summoned by a streaming auto-tail alone', () => {
    expect(nextIndicatorVisibility(false, ev({ programmaticScroll: true })))
      .toEqual({ shown: false, armHideTimer: false });
  });

  it('ignores the iOS compositor-recovery nudge entirely', () => {
    expect(nextIndicatorVisibility(false, ev({ repaintNudge: true })))
      .toEqual({ shown: false, armHideTimer: false });
    expect(nextIndicatorVisibility(true, ev({ repaintNudge: true })))
      .toEqual({ shown: true, armHideTimer: false });
  });

  it('still honours a real drag that races a nudge', () => {
    expect(nextIndicatorVisibility(false, ev({ repaintNudge: true, userScrolling: true })))
      .toEqual({ shown: true, armHideTimer: true });
  });

  it('stays down until something summons it', () => {
    expect(nextIndicatorVisibility(false, ev())).toEqual({ shown: false, armHideTimer: false });
  });

  it('arms the countdown for a drag that overlaps one of our own navigations', () => {
    expect(nextIndicatorVisibility(false, ev({ userScrolling: true, programmaticScroll: true })))
      .toEqual({ shown: true, armHideTimer: true });
  });

  it('arms the countdown for a drag that races a nudge during streaming', () => {
    expect(nextIndicatorVisibility(false, ev({
      userScrolling: true, programmaticScroll: true, repaintNudge: true,
    }))).toEqual({ shown: true, armHideTimer: true });
  });

  it('never turns the indicator on without starting the countdown that turns it off', () => {
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
              expect(userScrolling, `summoned by something other than a drag: ${label}`).toBe(true);
            }
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

  const CAPPED_PX = 600 * MAX_THUMB_FRACTION;
  const CROSSOVER = 800 / MAX_THUMB_FRACTION;

  it('is tuned so the cap is the binding limit on this fixture', () => {
    expect(CAPPED_PX).toBeGreaterThan(MIN_THUMB_PX);
  });

  it('stops a barely-overflowing thread filling the track', () => {
    const input = base({ scrollHeight: 810, clientHeight: 800, trackHeightPx: 600 });
    expect(computeScrollIndicator(input).thumbHeightPx).toBeCloseTo(CAPPED_PX, 5);
  });

  it('caps every thread shorter than the crossover at the same height', () => {
    for (const scrollHeight of [810, CROSSOVER * 0.5, CROSSOVER * 0.9, CROSSOVER - 1]) {
      const geo = computeScrollIndicator(base({ scrollHeight, clientHeight: 800, trackHeightPx: 600 }));
      expect(geo.thumbHeightPx, `scrollHeight=${scrollHeight}`).toBeCloseTo(CAPPED_PX, 5);
    }
  });

  it('goes back to being proportional once the thread passes the cap', () => {
    expect(computeScrollIndicator(base({ scrollHeight: CROSSOVER, clientHeight: 800, trackHeightPx: 600 })).thumbHeightPx)
      .toBeCloseTo(CAPPED_PX, 5);
    expect(computeScrollIndicator(base({ scrollHeight: CROSSOVER * 2, clientHeight: 800, trackHeightPx: 600 })).thumbHeightPx)
      .toBeCloseTo(CAPPED_PX / 2, 5);
  });

  it('still lands flush with both ends of the track when capped', () => {
    const input = base({ scrollHeight: 810, clientHeight: 800, trackHeightPx: 600 });
    expect(computeScrollIndicator({ ...input, scrollTop: 0 }).thumbOffsetPx).toBe(0);
    const atEnd = computeScrollIndicator({ ...input, scrollTop: 10 });
    expect(atEnd.thumbOffsetPx + atEnd.thumbHeightPx).toBeCloseTo(600, 5);
  });

  it('keeps the floor winning over the cap on a track too short for both', () => {
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
