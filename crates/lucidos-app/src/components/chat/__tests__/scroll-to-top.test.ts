import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';

import { mockScrollEl } from './scroll-test-helpers';
import {
  scrollToTop,
  scrollToBottom,
  scrollToBottomAnimated,
  setActiveScrollElement,
  makeScrollObservers,
  awayFromBottom,
  stopFollowingBottom,
} from '../scrollState';

// The chevron scroll is rAF-driven (vsync-aligned, time-based easeOutCubic). Fake
// timers fake requestAnimationFrame too, so advanceTimersByTime steps the frames.
describe('scrollToTop (rAF easeOutCubic)', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    awayFromBottom.value = false;
    setActiveScrollElement(null);
    stopFollowingBottom();
  });
  afterEach(() => {
    vi.clearAllTimers();
    vi.useRealTimers();
    setActiveScrollElement(null);
  });

  it('shows the down-chevron synchronously, from the first frame of the glide', () => {
    setActiveScrollElement(mockScrollEl({ scrollTop: 4000 }));
    scrollToTop();
    expect(awayFromBottom.value).toBe(true);
  });

  it('reacts immediately — the first frame takes a big step toward the top (responsive, not a slow drag)', () => {
    const el = mockScrollEl({ scrollTop: 4000, scrollHeight: 5000, clientHeight: 500 });
    setActiveScrollElement(el);

    scrollToTop();
    expect(el.scrollTop).toBe(4000); // nothing synchronous; motion is on rAF
    vi.advanceTimersByTime(20); // ~one frame
    expect(el.scrollTop).toBeLessThan(3700); // moved a substantial chunk at once
    expect(el.scrollTop).toBeGreaterThan(100); // but did not teleport to the top
  });

  it('decelerates toward the stop — early frames move more than late frames', () => {
    const el = mockScrollEl({ scrollTop: 4000, scrollHeight: 5000, clientHeight: 500 });
    setActiveScrollElement(el);

    scrollToTop();
    const pos: number[] = [el.scrollTop];
    for (let i = 0; i < 14; i++) {
      vi.advanceTimersByTime(16);
      pos.push(el.scrollTop);
    }
    const earlyMove = Math.abs(pos[2] - pos[1]);
    const lateMove = Math.abs(pos[12] - pos[11]);
    expect(earlyMove).toBeGreaterThan(lateMove);
  });

  it('lands exactly at the top once the animation settles', () => {
    const el = mockScrollEl({ scrollTop: 4000, scrollHeight: 5000, clientHeight: 500 });
    setActiveScrollElement(el);
    scrollToTop();
    vi.advanceTimersByTime(1500);
    expect(el.scrollTop).toBe(0);
  });

  it('still reaches the top even if scrollTop is nudged mid-animation (tracks its own state, no yield-bail)', () => {
    const el = mockScrollEl({ scrollTop: 4000, scrollHeight: 5000, clientHeight: 500 });
    setActiveScrollElement(el);

    scrollToTop();
    vi.advanceTimersByTime(48);
    expect(el.scrollTop).toBeLessThan(4000);

    el.scrollTop = 4000; // scroll-anchoring / late content settle shoves it
    vi.advanceTimersByTime(1500);
    expect(el.scrollTop).toBe(0);
  });

  it('reaches the top even straight after a go-to-bottom', () => {
    // The up-chevron renders the full windowed thread before scrolling, and the
    // huge ResizeObserver grow that follows used to hit the bottom-pin's
    // suppression window and slam the glide back down (the intermittent
    // "scroll-to-top lands mid/bottom" flake). Nothing pins on a resize now, so
    // the two directions cannot fight; the up-tap simply wins.
    const el = mockScrollEl({ scrollTop: 4000, scrollHeight: 5000, clientHeight: 500 });
    setActiveScrollElement(el);

    scrollToBottom();
    scrollToTop();
    vi.advanceTimersByTime(1500);
    expect(el.scrollTop).toBe(0);
  });

  it('reduced motion jumps instantly with no animation', () => {
    const realMatchMedia = window.matchMedia;
    (window as any).matchMedia = (q: string) => ({
      matches: q.includes('reduced-motion'),
      addEventListener: () => {}, removeEventListener: () => {},
      addListener: () => {}, removeListener: () => {}, dispatchEvent: () => false,
    });
    try {
      const el = mockScrollEl({ scrollTop: 4000, scrollHeight: 5000, clientHeight: 500 });
      setActiveScrollElement(el);
      scrollToTop();
      expect(el.scrollTop).toBe(0); // synchronous, no eased frames
    } finally {
      (window as any).matchMedia = realMatchMedia;
    }
  });
});

describe('scrollToBottomAnimated', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    awayFromBottom.value = true; // the reader is off the live edge, so the chevron shows
    setActiveScrollElement(null);
    // The standing follow a chevron tap arms is module state, so retire whatever
    // the previous case left armed (the same call `focusThread` makes).
    stopFollowingBottom();
  });
  afterEach(() => {
    vi.clearAllTimers();
    vi.useRealTimers();
    setActiveScrollElement(null);
  });

  it('eases to the bottom and hides the chevron on landing', () => {
    const el = mockScrollEl({ scrollTop: 0, scrollHeight: 5000, clientHeight: 500 });
    setActiveScrollElement(el);

    scrollToBottomAnimated();
    expect(el.scrollTop).toBe(0); // not synchronous
    vi.advanceTimersByTime(20);
    expect(el.scrollTop).toBeGreaterThan(0);

    vi.advanceTimersByTime(1500);
    // Lands on the maximum offset (scrollHeight - clientHeight), which IS the
    // bottom, and reconciles the chevron there. It used to hand off to a 500ms
    // pin loop for live tailing; tailing now rides the standing follow the
    // landing arms instead, which no loop and no timer participates in.
    expect(el.scrollTop).toBe(4500);
    expect(awayFromBottom.value).toBe(false);
  });

  it('lands on the TRUE bottom of content that grew during the glide', () => {
    // The target is re-read every frame, so a thread still streaming while the
    // tween runs is tracked rather than the tween landing on the bottom as it
    // was when the chevron was tapped.
    const el = mockScrollEl({ scrollTop: 0, scrollHeight: 5000, clientHeight: 500 });
    setActiveScrollElement(el);

    scrollToBottomAnimated();
    vi.advanceTimersByTime(100);
    el.scrollHeight = 9000; // three more turns arrive mid-glide

    vi.advanceTimersByTime(1500);
    expect(el.scrollTop).toBe(8500); // 9000 - 500, not 4500
    expect(awayFromBottom.value).toBe(false);
  });

  it('arms nothing once it lands: the tap asked for one jump', () => {
    const el = mockScrollEl({ scrollTop: 0, scrollHeight: 5000, clientHeight: 500 });
    const { onResize } = makeScrollObservers(el as any);
    setActiveScrollElement(el);

    scrollToBottomAnimated();
    vi.advanceTimersByTime(1500);
    expect(el.scrollTop).toBe(4500);

    // A beat later the reply streams on, and the reader stays exactly where the
    // tween put them. The landing used to ARM the standing follow, which left
    // the mode with no visible state and no way off; riding is the follow
    // toggle's own request now, and the chevron is a navigation like the up
    // chevron beside it (scroll-follow-the-live-edge.test.ts).
    el.scrollHeight = 6000;
    onResize();
    vi.advanceTimersByTime(1500);

    expect(el.scrollTop).toBe(4500);
    expect(awayFromBottom.value).toBe(true);
  });

  it('reduced motion snaps straight to the bottom (scrollToBottom) with no ease', () => {
    const realMatchMedia = window.matchMedia;
    (window as any).matchMedia = (q: string) => ({
      matches: q.includes('reduced-motion'),
      addEventListener: () => {}, removeEventListener: () => {},
      addListener: () => {}, removeListener: () => {}, dispatchEvent: () => false,
    });
    try {
      const el = mockScrollEl({ scrollTop: 0, scrollHeight: 5000, clientHeight: 500 });
      setActiveScrollElement(el);
      scrollToBottomAnimated();
      expect(el.scrollTop).toBe(5000); // synchronous snap, no eased frames
    } finally {
      (window as any).matchMedia = realMatchMedia;
    }
  });

  it('a down-tap cancels an in-flight scroll-to-top animation (mutually exclusive)', () => {
    const el = mockScrollEl({ scrollTop: 4000, scrollHeight: 5000, clientHeight: 500 });
    setActiveScrollElement(el);

    scrollToTop();
    vi.advanceTimersByTime(16); // top animation underway
    expect(el.scrollTop).toBeLessThan(4000);

    scrollToBottomAnimated();
    vi.advanceTimersByTime(1500);
    expect(el.scrollTop).toBe(4500); // went to the bottom, not dragged back to 0
  });
});
