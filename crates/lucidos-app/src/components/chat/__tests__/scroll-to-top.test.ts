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
import { osReducesMotion } from '../../../utils/motion';

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
    expect(el.scrollTop).toBe(4000);
    vi.advanceTimersByTime(20);
    expect(el.scrollTop).toBeLessThan(3700);
    expect(el.scrollTop).toBeGreaterThan(100);
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

    el.scrollTop = 4000;
    vi.advanceTimersByTime(1500);
    expect(el.scrollTop).toBe(0);
  });

  it('reaches the top even straight after a go-to-bottom', () => {
    const el = mockScrollEl({ scrollTop: 4000, scrollHeight: 5000, clientHeight: 500 });
    setActiveScrollElement(el);

    scrollToBottom();
    scrollToTop();
    vi.advanceTimersByTime(1500);
    expect(el.scrollTop).toBe(0);
  });

  it('reduced motion jumps instantly with no animation', () => {
    osReducesMotion.value = true;
    try {
      const el = mockScrollEl({ scrollTop: 4000, scrollHeight: 5000, clientHeight: 500 });
      setActiveScrollElement(el);
      scrollToTop();
      expect(el.scrollTop).toBe(0);
    } finally {
      osReducesMotion.value = false;
    }
  });
});

describe('scrollToBottomAnimated', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    awayFromBottom.value = true;
    setActiveScrollElement(null);
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
    expect(el.scrollTop).toBe(0);
    vi.advanceTimersByTime(20);
    expect(el.scrollTop).toBeGreaterThan(0);

    vi.advanceTimersByTime(1500);
    expect(el.scrollTop).toBe(4500);
    expect(awayFromBottom.value).toBe(false);
  });

  it('lands on the TRUE bottom of content that grew during the glide', () => {
    const el = mockScrollEl({ scrollTop: 0, scrollHeight: 5000, clientHeight: 500 });
    setActiveScrollElement(el);

    scrollToBottomAnimated();
    vi.advanceTimersByTime(100);
    el.scrollHeight = 9000;

    vi.advanceTimersByTime(1500);
    expect(el.scrollTop).toBe(8500);
    expect(awayFromBottom.value).toBe(false);
  });

  it('arms nothing once it lands: the tap asked for one jump', () => {
    const el = mockScrollEl({ scrollTop: 0, scrollHeight: 5000, clientHeight: 500 });
    const { onResize } = makeScrollObservers(el as any);
    setActiveScrollElement(el);

    scrollToBottomAnimated();
    vi.advanceTimersByTime(1500);
    expect(el.scrollTop).toBe(4500);

    el.scrollHeight = 6000;
    onResize();
    vi.advanceTimersByTime(1500);

    expect(el.scrollTop).toBe(4500);
    expect(awayFromBottom.value).toBe(true);
  });

  it('reduced motion snaps straight to the bottom (scrollToBottom) with no ease', () => {
    osReducesMotion.value = true;
    try {
      const el = mockScrollEl({ scrollTop: 0, scrollHeight: 5000, clientHeight: 500 });
      setActiveScrollElement(el);
      scrollToBottomAnimated();
      expect(el.scrollTop).toBe(5000);
    } finally {
      osReducesMotion.value = false;
    }
  });

  it('a down-tap cancels an in-flight scroll-to-top animation (mutually exclusive)', () => {
    const el = mockScrollEl({ scrollTop: 4000, scrollHeight: 5000, clientHeight: 500 });
    setActiveScrollElement(el);

    scrollToTop();
    vi.advanceTimersByTime(16);
    expect(el.scrollTop).toBeLessThan(4000);

    scrollToBottomAnimated();
    vi.advanceTimersByTime(1500);
    expect(el.scrollTop).toBe(4500);
  });
});
