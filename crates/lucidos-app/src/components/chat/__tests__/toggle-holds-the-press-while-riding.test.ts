import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';

// Stub HTMLElement before importing the modules that reference it, exactly as
// the sibling scroll suites do.
if (typeof (globalThis as any).HTMLElement === 'undefined') {
  (globalThis as any).HTMLElement = class {};
}
if (typeof (globalThis as any).MutationObserver === 'undefined') {
  // `withScrollAnchor` observes the container so a Preact render that commits
  // asynchronously still gets its correction. Nothing here needs the callback
  // to fire: the anchor moves inside `fn`, which takes the synchronous path.
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

/**
 * **A turn control holds what was pressed, riding or not (ADR 0147).**
 *
 * Toggling *full response* or *steps* grows or shrinks every turn in the
 * transcript. `withScrollAnchor` holds the control the reader pressed while
 * the DOM changes under them, whatever the follow is doing.
 *
 * A reader riding the live edge used to be carried to the new edge instead,
 * which moved the icon they had just pressed. Now the press wins. Where the
 * hold leaves them off the live edge, the ride ends with it. The toggle is
 * never lit over a reader it no longer holds (ADR 0064).
 */
describe('a turn-control toggle holds the pressed control, riding or not', () => {
  /** A transcript double: enough of an element for the anchor machinery, and a
   *  record of every distinct position written to it. */
  function makeContainer(opts: { scrollTop: number; scrollHeight: number; clientHeight: number }) {
    const el: any = {
      isConnected: true,
      parentElement: null,
      children: [],
      style: mockStyle({ overflow: '', transform: '' }),
      clientWidth: 800,
      clientHeight: opts.clientHeight,
      offsetHeight: opts.clientHeight,
      /** Every position the container was left at, in order. A write of the
       *  value it already holds moves nobody, so it is not recorded: what is
       *  being counted is motion the reader can see. */
      settled: [] as number[],
      _scrollTop: opts.scrollTop,
      _scrollHeight: opts.scrollHeight,
      get scrollTop() { return this._scrollTop; },
      set scrollTop(v: number) {
        const clamped = Math.min(Math.max(0, v), Math.max(0, this.scrollHeight - this.clientHeight));
        if (clamped !== this._scrollTop) this.settled.push(clamped);
        this._scrollTop = clamped;
      },
      /** Content SHRINKING re-clamps the offset, exactly as a browser does. A
       *  shrink therefore records the clamp as a motion of its own. */
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

  /** The turn the reader's tap grew, as `withScrollAnchor` sees it.
   *
   *  It answers `getBoundingClientRect` too, derived from `offsetTop` and the
   *  container's current scroll. The correction is measured through rects
   *  rather than the platform's whole-pixel `offsetTop` (see
   *  `contentOffsetTop`). Each case still MOVES the turn by assigning
   *  `offsetTop`, and the rect follows from it. */
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

  /** Hiding the steps: every turn in the transcript shrinks, so the content
   *  gets shorter and the toggled turn's own top rises a long way. The shrink's
   *  clamp lands the reader on 2300, and holding the control takes them to
   *  1300. */
  function hideSteps(el: any) {
    const anchor = makeAnchor(el, 2000);
    withScrollAnchor(anchor, () => {
      (anchor as any).offsetTop = 800;   // the turn's top rises by 1200
      el.scrollHeight = 2800;            // and the transcript loses 200 of height
    });
    vi.advanceTimersByTime(1500);
  }

  /** Showing the steps: every turn in the transcript grows, so the transcript
   *  gets taller, the toggled turn's own top falls, and the live edge moves
   *  DOWN. Holding the control leaves the reader at 3700, short of 4500. */
  function showSteps(el: any) {
    const anchor = makeAnchor(el, 800);
    withScrollAnchor(anchor, () => {
      (anchor as any).offsetTop = 2000;  // the turn's top falls by 1200
      el.scrollHeight = 5000;            // and the transcript gains 2000 of height
    });
    vi.advanceTimersByTime(1500);
  }

  /** An armed reader, riding the live edge of this 3000px transcript. */
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

    // 3700 holds the pressed control: its top fell 1200, so the offset rises by
    // the same. 4500 is the live-edge snap they are no longer given.
    expect(el.settled).toEqual([3700]);
    expect(followingLiveEdge.value).toBe(false);
  });

  it('holds it when the steps go off, undoing the shrink clamp', () => {
    const el = riding();

    hideSteps(el);

    // 2300 is the shrink's own clamp, which nobody chose; 1300 puts the pressed
    // control back where it was.
    expect(el.settled).toEqual([2300, 1300]);
    expect(followingLiveEdge.value).toBe(false);
  });

  it('keeps the ride when holding the control leaves the reader on the edge', () => {
    // Everything grew ABOVE the control, so the reader moves with it and is
    // still on the live edge.
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
    // A transcript SHORTER than its pane has no overflow to be at the edge of,
    // and the reveal is exactly what gives it one. The control still holds.
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

    // 800 holds the control; 4500 is the end of the thread it is nowhere near.
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
    // The freeze is what stops the browser adjusting the scroll mid-mutation.
    // Leaving it on would make the transcript unscrollable.
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
