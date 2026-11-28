import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';

if (typeof (globalThis as any).HTMLElement === 'undefined') {
  (globalThis as any).HTMLElement = class {};
}
if (typeof (globalThis as any).MutationObserver === 'undefined') {
  (globalThis as any).MutationObserver = class {
    observe() {}
    disconnect() {}
  };
}

import { withScrollAnchor } from '../CreateThreadView';
import { mockStyle } from './scroll-test-helpers';
import {
  followingLiveEdge,
  setActiveScrollElement,
  setFollowLiveEdge,
  stopFollowingBottom,
} from '../scrollState';

describe('a turn-control toggle holds the pressed control, riding or not', () => {
  function makeContainer(opts: { scrollTop: number; scrollHeight: number; clientHeight: number }) {
    const el: any = {
      isConnected: true,
      parentElement: null,
      children: [],
      style: mockStyle({ overflow: '', transform: '' }),
      clientWidth: 800,
      clientHeight: opts.clientHeight,
      offsetHeight: opts.clientHeight,
      settled: [] as number[],
      _scrollTop: opts.scrollTop,
      _scrollHeight: opts.scrollHeight,
      get scrollTop() { return this._scrollTop; },
      set scrollTop(v: number) {
        const clamped = Math.min(Math.max(0, v), Math.max(0, this.scrollHeight - this.clientHeight));
        if (clamped !== this._scrollTop) this.settled.push(clamped);
        this._scrollTop = clamped;
      },
      get scrollHeight() { return this._scrollHeight; },
      set scrollHeight(v: number) {
        this._scrollHeight = v;
        const max = Math.max(0, v - this.clientHeight);
        if (this._scrollTop > max) {
          this._scrollTop = max;
          this.settled.push(max);
        }
      },
      getBoundingClientRect: () => ({
        width: 800, height: el.clientHeight, top: 0, bottom: el.clientHeight, left: 0, right: 800,
      }),
      querySelectorAll: () => [],
    };
    return el;
  }

  function makeAnchor(container: any, offsetTop: number) {
    const a: any = {
      isConnected: true,
      offsetTop,
      closest: (sel: string) => (sel === '.thread-content' ? container : null),
      getBoundingClientRect: () => {
        const top = container.getBoundingClientRect().top + a.offsetTop - container.scrollTop;
        return { width: 800, height: 0, top, bottom: top, left: 0, right: 800 };
      },
    };
    return a as unknown as Element;
  }

  beforeEach(() => {
    stopFollowingBottom();
    setActiveScrollElement(null);
    vi.useFakeTimers();
  });
  afterEach(() => {
    vi.clearAllTimers();
    vi.useRealTimers();
    stopFollowingBottom();
    setActiveScrollElement(null);
  });

  function hideSteps(el: any) {
    const anchor = makeAnchor(el, 2000);
    withScrollAnchor(anchor, () => {
      (anchor as any).offsetTop = 800;
      el.scrollHeight = 2800;
    });
    vi.advanceTimersByTime(1500);
  }

  function showSteps(el: any) {
    const anchor = makeAnchor(el, 800);
    withScrollAnchor(anchor, () => {
      (anchor as any).offsetTop = 2000;
      el.scrollHeight = 5000;
    });
    vi.advanceTimersByTime(1500);
  }

  function riding() {
    const el = makeContainer({ scrollTop: 2500, scrollHeight: 3000, clientHeight: 500 });
    setActiveScrollElement(el);
    setFollowLiveEdge(true);
    vi.advanceTimersByTime(1500);
    expect(followingLiveEdge.value).toBe(true);
    el.settled.length = 0;
    return el;
  }

  it('holds the control when the steps go on, and ends the ride it left short', () => {
    const el = riding();

    showSteps(el);

    expect(el.settled).toEqual([3700]);
    expect(followingLiveEdge.value).toBe(false);
  });

  it('holds it when the steps go off, undoing the shrink clamp', () => {
    const el = riding();

    hideSteps(el);

    expect(el.settled).toEqual([2300, 1300]);
    expect(followingLiveEdge.value).toBe(false);
  });

  it('keeps the ride when holding the control leaves the reader on the edge', () => {
    const el = riding();

    const anchor = makeAnchor(el, 2600);
    withScrollAnchor(anchor, () => {
      (anchor as any).offsetTop = 3800;
      el.scrollHeight = 4200;
    });
    vi.advanceTimersByTime(1500);

    expect(el.settled).toEqual([3700]);
    expect(followingLiveEdge.value).toBe(true);
  });

  it('holds it when the reveal makes a short thread scrollable', () => {
    const el = makeContainer({ scrollTop: 0, scrollHeight: 400, clientHeight: 500 });
    setActiveScrollElement(el);
    setFollowLiveEdge(true);
    vi.advanceTimersByTime(1500);
    el.settled.length = 0;

    const anchor = makeAnchor(el, 100);
    withScrollAnchor(anchor, () => {
      (anchor as any).offsetTop = 900;
      el.scrollHeight = 5000;
    });
    vi.advanceTimersByTime(1500);

    expect(el.settled).toEqual([800]);
    expect(followingLiveEdge.value).toBe(false);
  });

  it('gives a reader who is NOT riding the same correction', () => {
    const el = makeContainer({ scrollTop: 2500, scrollHeight: 3000, clientHeight: 500 });
    setActiveScrollElement(el);

    showSteps(el);

    expect(el.settled).toEqual([3700]);
  });

  it('unfreezes the container either way', () => {
    for (const armed of [true, false]) {
      stopFollowingBottom();
      const el = makeContainer({ scrollTop: 2500, scrollHeight: 3000, clientHeight: 500 });
      setActiveScrollElement(el);
      if (armed) {
        setFollowLiveEdge(true);
        vi.advanceTimersByTime(1500);
      }

      const anchor = makeAnchor(el, 1000);
      withScrollAnchor(anchor, () => { (anchor as any).offsetTop = 1600; });
      vi.advanceTimersByTime(1500);

      expect(el.style.overflow, `armed=${armed}`).toBe('');
    }
  });
});
