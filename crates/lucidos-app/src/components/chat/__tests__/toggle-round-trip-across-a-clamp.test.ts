import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

// Stub HTMLElement before importing the modules that reference it, exactly as
// the sibling scroll suites do.
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
import { setActiveScrollElement, setThreadLive, stopFollowingBottom } from '../scrollState';

/**
 * **A reveal is reversible, even when the clamp eats part of the correction.**
 *
 * Turning the full response off removes most of the transcript, and with less
 * content below the anchored turn than the viewport is tall, no offset puts
 * that turn back where it was: the browser clamps and the turn slides. That
 * much is geometry and no correction can undo it.
 *
 * The ROUND TRIP is not geometry. The reverse toggle restores its own delta
 * from wherever the clamp left the reader, so without a memory it lands short
 * by exactly what the clamp ate, and every pair of taps drifts again. That is
 * the "slight jump up and down" the user reported on 2026-08-11, and it is why
 * it never happens on a thread too short to scroll: with nothing to clamp,
 * every correction is reachable.
 *
 * The numbers here are measured, not invented. A seeded six-turn thread in
 * Chromium (crates/lucidos-app e2e, 573px viewport): the answer-only view took
 * the transcript from 2737 to 1915, the reader sat at 2124, the anchored turn's
 * `offsetTop` went 2279 to 1594, so the correction wanted 1439 against a new
 * maximum of 1342. The growth direction measured pixel-exact in the same run,
 * which is why only the shrink needs a memory.
 */
describe('a turn-control toggle returns the reader across a clamp', () => {
  const VIEWPORT = 573;
  const TALL = 2737;
  const SHORT = 1915;
  const ANCHOR_TALL = 2279;
  const ANCHOR_SHORT = 1594;
  /** The reader's offset before the first tap, as the measurement had it. */
  const START = 2124;
  /** The most the shrunk transcript can be scrolled to. */
  const SHORT_MAX = SHORT - VIEWPORT; // 1342

  function makeContainer(scrollTop: number, scrollHeight: number) {
    const listeners = new Set<() => void>();
    const el: any = {
      isConnected: true,
      style: { overflow: '' },
      clientHeight: VIEWPORT,
      clientWidth: 800,
      offsetHeight: VIEWPORT,
      children: [],
      _scrollTop: scrollTop,
      _scrollHeight: scrollHeight,
      // A real container dispatches `scroll` for every offset change, its own
      // writes included, which is what the debt's watcher reads.
      addEventListener(type: string, fn: () => void) { if (type === 'scroll') listeners.add(fn); },
      removeEventListener(type: string, fn: () => void) { if (type === 'scroll') listeners.delete(fn); },
      get scrollTop() { return this._scrollTop; },
      set scrollTop(v: number) {
        const next = Math.min(Math.max(0, v), Math.max(0, this.scrollHeight - this.clientHeight));
        if (next === this._scrollTop) return;
        this._scrollTop = next;
        for (const fn of Array.from(listeners)) fn();
      },
      get scrollHeight() { return this._scrollHeight; },
      set scrollHeight(v: number) {
        this._scrollHeight = v;
        // The browser re-clamps when content shrinks under the reader.
        const max = Math.max(0, v - this.clientHeight);
        if (this._scrollTop > max) this._scrollTop = max;
      },
      getBoundingClientRect: () => ({ width: 800, height: VIEWPORT, top: 0, bottom: VIEWPORT, left: 0, right: 800 }),
      querySelectorAll: () => [],
    };
    return el;
  }

  /** It answers `getBoundingClientRect` as well as `offsetTop`, derived from that
   *  offset and the container's current scroll, because the correction is
   *  measured through rects rather than the platform's whole-pixel `offsetTop`
   *  (see `contentOffsetTop`). Each case still MOVES the turn by assigning
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

  /** One tap: the transcript changes height and the anchored turn's top moves. */
  function toggle(el: any, anchor: any, nextAnchorTop: number, nextHeight: number) {
    withScrollAnchor(anchor, () => {
      anchor.offsetTop = nextAnchorTop;
      el.scrollHeight = nextHeight;
    });
    vi.advanceTimersByTime(1500);
  }

  beforeEach(() => {
    stopFollowingBottom();
    setActiveScrollElement(null);
    setThreadLive(false);
    vi.useFakeTimers();
  });
  afterEach(() => {
    vi.clearAllTimers();
    vi.useRealTimers();
    stopFollowingBottom();
    setActiveScrollElement(null);
  });

  it('lands where it can on the way in, and exactly back on the way out', () => {
    const el = makeContainer(START, TALL);
    const anchor: any = makeAnchor(el, ANCHOR_TALL);
    setActiveScrollElement(el);

    toggle(el, anchor, ANCHOR_SHORT, SHORT);
    // The correction wanted 1439 and the transcript ends at 1342.
    expect(el.scrollTop).toBe(SHORT_MAX);

    toggle(el, anchor, ANCHOR_TALL, TALL);
    // Back exactly, rather than 97px short of it.
    expect(el.scrollTop).toBe(START);
  });

  it('pays the debt once, not on every later reveal', () => {
    const el = makeContainer(START, TALL);
    const anchor: any = makeAnchor(el, ANCHOR_TALL);
    setActiveScrollElement(el);

    toggle(el, anchor, ANCHOR_SHORT, SHORT);
    toggle(el, anchor, ANCHOR_TALL, TALL);
    expect(el.scrollTop).toBe(START);

    // A third tap is an ordinary reveal again: same shrink, same clamp, and no
    // stale credit inflating it.
    toggle(el, anchor, ANCHOR_SHORT, SHORT);
    expect(el.scrollTop).toBe(SHORT_MAX);
  });

  it('drops the debt the moment the reader scrolls', () => {
    // The clamp was forced on us; a scroll is the reader choosing a position,
    // and it is theirs. Repaying afterwards would move them off it.
    const el = makeContainer(START, TALL);
    const anchor: any = makeAnchor(el, ANCHOR_TALL);
    setActiveScrollElement(el);

    toggle(el, anchor, ANCHOR_SHORT, SHORT);
    expect(el.scrollTop).toBe(SHORT_MAX);

    el.scrollTop = 400; // the reader flicks up
    toggle(el, anchor, ANCHOR_TALL, TALL);

    expect(el.scrollTop).toBe(400 + (ANCHOR_TALL - ANCHOR_SHORT));
  });

  it('drops it even when the reader scrolls away and comes back to the same offset', () => {
    // The clamped offset IS the live edge of the shrunk transcript, so scrolling
    // up to read and then back to the bottom lands on it to the pixel. Comparing
    // offsets at the next reveal cannot tell that apart from never having moved,
    // which is why the debt watches the scroll EVENT: the trip away is what
    // retires it, and the bottom they chose to return to is theirs.
    const el = makeContainer(START, TALL);
    const anchor: any = makeAnchor(el, ANCHOR_TALL);
    setActiveScrollElement(el);

    toggle(el, anchor, ANCHOR_SHORT, SHORT);
    expect(el.scrollTop).toBe(SHORT_MAX);

    el.scrollTop = 400;        // the reader flicks up to re-read something
    el.scrollTop = SHORT_MAX;  // and scrolls back down to the end

    toggle(el, anchor, ANCHOR_TALL, TALL);
    expect(el.scrollTop).toBe(SHORT_MAX + (ANCHOR_TALL - ANCHOR_SHORT));
  });

  it('does not pay a debt earned in another thread', () => {
    // The transcript ELEMENT is reused across threads, so an offset restored on
    // the way into another thread can land on the one a clamp left behind here.
    // The content height is the other half of the test, and a different thread
    // does not match it.
    const el = makeContainer(START, TALL);
    const anchor: any = makeAnchor(el, ANCHOR_TALL);
    setActiveScrollElement(el);

    toggle(el, anchor, ANCHOR_SHORT, SHORT);
    expect(el.scrollTop).toBe(SHORT_MAX);

    el.scrollHeight = 4000;      // another thread's transcript
    el.scrollTop = SHORT_MAX;    // restored, by coincidence, to the same offset
    toggle(el, anchor, ANCHOR_TALL, TALL);

    expect(el.scrollTop).toBe(SHORT_MAX + (ANCHOR_TALL - ANCHOR_SHORT));
  });

  it('derives the clamp from the extent, never from reading the write back', () => {
    // `wanted - container.scrollTop` is ONE read rather than a difference of
    // two, so it carries whatever the engine answers with. Reading `scrollTop`
    // in the write's own task, on a container whose children just changed, does
    // not reliably answer the new value. On WebKit that measured a debt nobody
    // was owed. The reverse press then paid out several hundred pixels of it,
    // in one run out of a few.
    //
    // A source scan rather than a behavioural test. The failure is a browser's
    // own timing, which this suite's honest container cannot produce. Every
    // model of it that could was a fiction the rest of the harness saw through.
    // What IS checkable is that no read-back is used.
    const source = readFileSync(
      resolve(dirname(fileURLToPath(import.meta.url)), '../CreateThreadView.tsx'),
      'utf8',
    );
    expect(source).toMatch(/const landed = landingScrollTop\(container, wanted\);/);
    expect(source).not.toMatch(/rememberAnchorDebt\([^)]*container\.scrollTop/);
  });

  it('a transcript too short to scroll never moves at all', () => {
    // The user's own discriminator: no scrollable content, no jump. Nothing to
    // clamp means nothing to remember either.
    const el = makeContainer(0, VIEWPORT);
    const anchor: any = makeAnchor(el, 30);
    setActiveScrollElement(el);

    toggle(el, anchor, 30, VIEWPORT);
    expect(el.scrollTop).toBe(0);

    toggle(el, anchor, 30, VIEWPORT);
    expect(el.scrollTop).toBe(0);
  });
});
