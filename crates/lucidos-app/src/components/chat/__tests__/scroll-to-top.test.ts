import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';

import { mockScrollEl } from './scroll-test-helpers';
import {
  scrollToTop,
  scrollToBottom,
  scrollToBottomAnimated,
  setActiveScrollElement,
  getResizeMode,
  scrolledUp,
  awayFromBottom,
} from '../scrollState';

// The chevron scroll is rAF-driven (vsync-aligned, time-based easeOutCubic). Fake
// timers fake requestAnimationFrame too, so advanceTimersByTime steps the frames.
describe('scrollToTop (rAF easeOutCubic)', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    scrolledUp.value = false;
    awayFromBottom.value = false;
    setActiveScrollElement(null);
  });
  afterEach(() => {
    vi.clearAllTimers();
    vi.useRealTimers();
    setActiveScrollElement(null);
  });

  it('marks scrolled-up state synchronously so auto-scroll-to-bottom defers and the down-chevron shows', () => {
    setActiveScrollElement(mockScrollEl({ scrollTop: 4000 }));
    scrollToTop();
    expect(scrolledUp.value).toBe(true);
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

  it('forces resize mode to ignore even mid scroll-to-bottom, so a render-all grow cannot pin back to bottom', () => {
    const el = mockScrollEl({ scrollTop: 4000, scrollHeight: 5000, clientHeight: 500 });
    setActiveScrollElement(el);

    scrollToBottom();
    expect(getResizeMode()).toBe('scroll');

    scrollToTop();
    expect(getResizeMode()).toBe('ignore'); // set synchronously
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
    scrolledUp.value = true; // user has scrolled up (the only time the down-chevron shows)
    awayFromBottom.value = true;
    setActiveScrollElement(null);
  });
  afterEach(() => {
    vi.clearAllTimers();
    vi.useRealTimers();
    setActiveScrollElement(null);
  });

  it('eases toward the bottom, then hands off to live tailing', () => {
    const el = mockScrollEl({ scrollTop: 0, scrollHeight: 5000, clientHeight: 500 });
    setActiveScrollElement(el);

    scrollToBottomAnimated();
    expect(el.scrollTop).toBe(0); // not synchronous
    vi.advanceTimersByTime(20);
    expect(el.scrollTop).toBeGreaterThan(0);

    vi.advanceTimersByTime(1500);
    // Handed off to scrollToBottom(): pinned to the live bottom + tailing engaged.
    // (scrolledUp=false is the tailing signal; _resizeMode has since decayed from
    // 'scroll' back to 'ignore' as scrollToBottom's 500ms suppression window
    // expired — that's expected, not a missed handoff.)
    expect(el.scrollTop).toBe(5000);
    expect(scrolledUp.value).toBe(false);
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
      expect(el.scrollTop).toBe(5000); // synchronous snap
      expect(getResizeMode()).toBe('scroll');
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
    expect(el.scrollTop).toBe(5000); // went to the bottom, not dragged back to 0
  });
});
