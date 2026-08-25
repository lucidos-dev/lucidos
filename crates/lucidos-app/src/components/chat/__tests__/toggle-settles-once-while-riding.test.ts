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
import {
  followingLiveEdge,
  setActiveScrollElement,
  setFollowLiveEdge,
  setThreadLive,
  stopFollowingBottom,
} from '../scrollState';

/**
 * **One tap, one settled position.**
 *
 * Toggling *full response* or *steps* grows or shrinks every turn in the
 * transcript, so `withScrollAnchor` holds the reader on the content they were
 * looking at while the DOM changes under them. That is right for a reader who
 * is reading, and it is precisely backwards for one RIDING the live edge, who
 * has asked to be kept on the newest content instead.
 *
 * Doing both was the bug: the correction moved them to hold the anchor, and
 * `honourAnchoredMutation` then brought them back down to the live edge, so one
 * tap on a live thread visibly scrolled the transcript UP and then DOWN again.
 * The freeze (`overflow: hidden`) has kept the container still through the
 * mutation, so skipping the correction for a riding reader leaves the live-edge
 * write as the tap's one motion.
 *
 * Both DIRECTIONS are here, because they fail differently and the second was
 * reported after the first was fixed. Hiding the steps SHRINKS the transcript,
 * and the browser's own clamp carries a riding reader to the new live edge, so
 * the only thing to get wrong is writing something else afterwards. Showing them
 * GROWS it, nothing clamps, and the reader is left exactly as far short of the
 * live edge as the growth above them: that gap is what a tween used to animate
 * away, which reads as the transcript jumping up and then scrolling itself.
 */
describe('a turn-control toggle settles once while riding the live edge', () => {
  /** A transcript double: enough of an element for the anchor machinery, and a
   *  record of every distinct position written to it. */
  function makeContainer(opts: { scrollTop: number; scrollHeight: number; clientHeight: number }) {
    const el: any = {
      isConnected: true,
      parentElement: null,
      children: [],
      style: { overflow: '', transform: '' },
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
      /** Content SHRINKING re-clamps the offset, exactly as a browser does, and
       *  that is load-bearing here: a reader riding the live edge is carried to
       *  the new one by the clamp alone, so any write after it is a second,
       *  visible motion rather than the same one. */
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
   *  container's current scroll, because the correction is measured through
   *  rects rather than the platform's whole-pixel `offsetTop` (see
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
    setThreadLive(true);
    vi.useFakeTimers();
  });
  afterEach(() => {
    vi.clearAllTimers();
    vi.useRealTimers();
    stopFollowingBottom();
    setActiveScrollElement(null);
  });

  /** Hiding the steps: every turn in the transcript shrinks, so the content
   *  gets shorter and the toggled turn's own top rises a long way. This is the
   *  direction the report came from, and the only one where the anchor
   *  correction points the OPPOSITE way to the ride: the shrink's clamp has
   *  already carried a riding reader to the new live edge (2300), and
   *  correcting to hold the anchor would send them back up to 1300 before the
   *  glide brought them down again. */
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
   *  DOWN. Nothing clamps this direction, so it is the one where the reader is
   *  left short of the edge and has to be put back on it. */
  function showSteps(el: any) {
    const anchor = makeAnchor(el, 800);
    withScrollAnchor(anchor, () => {
      (anchor as any).offsetTop = 2000;  // the turn's top falls by 1200
      el.scrollHeight = 5000;            // and the transcript gains 2000 of height
    });
    vi.advanceTimersByTime(1500);
  }

  it('shows the steps without leaving the reader short of the live edge first', () => {
    const el = makeContainer({ scrollTop: 2500, scrollHeight: 3000, clientHeight: 500 });
    setActiveScrollElement(el);
    setFollowLiveEdge(true);
    vi.advanceTimersByTime(1500);
    expect(followingLiveEdge.value).toBe(true);
    el.settled.length = 0;

    showSteps(el);

    // ONE settle, on the new live edge, written inside the frame that unfroze
    // the container. A tween here was the report: the reader sat at 2500 with
    // 2000px of new content below them, a whole screenful and more short of the
    // edge, and the transcript then scrolled itself down through every eased
    // frame in between.
    expect(el.settled).toEqual([4500]);
    expect(followingLiveEdge.value).toBe(true);
  });

  it('leaves a reader who is NOT riding exactly where the correction put them', () => {
    // The same growth, the other reader: they get the anchor correction (the
    // toggled turn's top fell 1200, so the offset rises by 1200 to keep it under
    // their eyes) and nothing else.
    const el = makeContainer({ scrollTop: 2500, scrollHeight: 3000, clientHeight: 500 });
    setActiveScrollElement(el);
    expect(followingLiveEdge.value).toBe(false);
    el.settled.length = 0;

    showSteps(el);

    expect(el.settled).toEqual([3700]);
  });

  it('lands on the live edge, without the trip back up to the anchor first', () => {
    const el = makeContainer({ scrollTop: 2500, scrollHeight: 3000, clientHeight: 500 });
    setActiveScrollElement(el);
    setFollowLiveEdge(true);
    vi.advanceTimersByTime(1500);
    expect(followingLiveEdge.value).toBe(true);
    el.settled.length = 0;

    hideSteps(el);

    // The clamp, and nothing after it. 1300 is what the anchor correction would
    // have written (2500 less the 1200 the anchor rose), which is 1000px ABOVE
    // where the shrink already left them: the transcript would have gone up
    // there and then glided back down, for one tap.
    expect(el.settled).toEqual([2300]);
    expect(el.settled).not.toContain(1300);
    expect(followingLiveEdge.value).toBe(true);
  });

  it('still holds a reader who is NOT riding on the content they were reading', () => {
    // The other half of the contract, and the reason the correction exists at
    // all: a reader who never asked to follow anything must not be moved by a
    // disclosure, whatever it grew or shrank. They get the correction, and they
    // are the only ones who do.
    const el = makeContainer({ scrollTop: 2500, scrollHeight: 3000, clientHeight: 500 });
    setActiveScrollElement(el);
    expect(followingLiveEdge.value).toBe(false);
    el.settled.length = 0;

    hideSteps(el);

    // 2300 is the shrink's own clamp; 1300 is the correction that puts the
    // toggled turn back under their eyes.
    expect(el.settled).toEqual([2300, 1300]);
  });

  it('unfreezes the container either way', () => {
    // The freeze is what stops the browser adjusting the scroll mid-mutation.
    // Leaving it on would make the transcript unscrollable, so the riding path
    // has to restore it exactly as the correcting path does.
    for (const riding of [true, false]) {
      stopFollowingBottom();
      const el = makeContainer({ scrollTop: 2500, scrollHeight: 3000, clientHeight: 500 });
      setActiveScrollElement(el);
      if (riding) {
        setFollowLiveEdge(true);
        vi.advanceTimersByTime(1500);
      }

      const anchor = makeAnchor(el, 1000);
      withScrollAnchor(anchor, () => { (anchor as any).offsetTop = 1600; });
      vi.advanceTimersByTime(1500);

      expect(el.style.overflow, `riding=${riding}`).toBe('');
    }
  });

  /** **Clicking around an IDLE thread.**
   *
   *  The block above is the LIVE case, where "riding" and "reading" genuinely
   *  conflict: content is arriving and an armed reader asked for the newest of
   *  it. On an idle thread nothing is arriving, so the ride carries nobody, and
   *  a reader parked in HISTORY gets the correction. They expanded a turn and
   *  want to see what they expanded.
   *
   *  A reader sitting ON the live edge is the exception, and the two answers
   *  cannot both be given. Rows revealed between their topmost line and the end
   *  of the thread push that end off the bottom. So holding the line is what
   *  moves them. ADR 0064 decides it: an armed reader on the live edge is kept
   *  there, running thread or quiet one.
   *
   *  BOTH halves have to agree, or the reader gets neither treatment. The snap
   *  lives in `honourAnchoredMutation` and the skip lives in `withScrollAnchor`.
   *  Skip one without the other and the reader is frozen at their old offset
   *  with the content above them grown, which is a drift. So the caller asks
   *  `readerKeepsTheLiveEdge` once, before the mutation, and hands that one
   *  answer to the other half.
   *
   *  A reveal is GROWTH too, so the resize handler runs for one. Its growth
   *  branch keeps an armed reader who is still ON the live edge there
   *  (`keepTheLiveEdge`). The two agree by construction: they read the same two
   *  terms. This file drives the two anchor halves directly, so the cases below
   *  are the correction's own answer rather than the whole app's. */
  describe('and keeps an armed reader on the end of an IDLE thread', () => {
    /** Arm the follow, then let the thread go quiet. */
    function armedOnAFinishedThread(scrollTop = 2500) {
      const el = makeContainer({ scrollTop: 3000, scrollHeight: 3000, clientHeight: 500 });
      setActiveScrollElement(el);
      setFollowLiveEdge(true);
      vi.advanceTimersByTime(1500);
      expect(followingLiveEdge.value).toBe(true);
      setThreadLive(false);
      el.scrollTop = scrollTop;
      el.settled.length = 0;
      return el;
    }

    it('keeps the newest content under their eye when the steps go on', () => {
      const el = armedOnAFinishedThread();

      showSteps(el);

      // 4500, the new live edge, in ONE write. Not 3700: holding their topmost
      // line would push the end of the thread 800px below the fold, which is
      // the report this case came from.
      expect(el.settled).toEqual([4500]);
      expect(el.settled).not.toContain(3700);
    });

    it('lets the shrink carry them when the steps go off', () => {
      const el = armedOnAFinishedThread();

      hideSteps(el);

      // The shrink's own clamp put them on the new end, so there is nothing
      // left to write. 1300 is the correction they are NOT given.
      expect(el.settled).toEqual([2300]);
      expect(el.settled).not.toContain(1300);
    });

    it('still corrects an armed reader parked back in history', () => {
      // The edge term is what makes this different, and it is why the ride's
      // own flag cannot answer alone. Nothing is arriving, they are nowhere
      // near the end, and the turn they expanded stays under their eye.
      const el = armedOnAFinishedThread(1200);

      showSteps(el);

      expect(el.settled).toEqual([2400]);
      expect(el.settled).not.toContain(4500);
    });

    it('keeps the end when the reveal makes a short thread scrollable', () => {
      // A transcript SHORTER than its pane has no overflow to be at the edge
      // of, and the reveal is exactly what gives it one. An `isScrollable`
      // term would drop the request for precisely that reader.
      const el = makeContainer({ scrollTop: 0, scrollHeight: 400, clientHeight: 500 });
      setActiveScrollElement(el);
      setFollowLiveEdge(true);
      vi.advanceTimersByTime(1500);
      setThreadLive(false);
      el.settled.length = 0;

      const anchor = makeAnchor(el, 100);
      withScrollAnchor(anchor, () => {
        (anchor as any).offsetTop = 900;
        el.scrollHeight = 5000;
      });
      vi.advanceTimersByTime(1500);

      // The new end, not the 800 the anchor correction would have held them at.
      expect(el.settled).toEqual([4500]);
    });

    it('keeps the ride armed through all of it', () => {
      // Nothing here is a retirement. The toggle stays lit, and the next live
      // turn carries them again.
      const el = armedOnAFinishedThread();

      showSteps(el);

      expect(followingLiveEdge.value).toBe(true);
    });
  });
});
