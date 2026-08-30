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
 * **One press reads only its own two measurements.**
 *
 * Hiding the steps at the end of a thread removes content BELOW the pressed
 * control. With less left under it than the viewport is tall, no offset puts
 * that control back, so the browser clamps and it slides. That is geometry, and
 * no correction undoes it.
 *
 * What the reverse press must NOT do is undo it later. The deficit was once
 * remembered against the pressed element and added back by the next press of
 * it. That threw the pressed control up the screen and left the reader on the
 * live edge, reported as the step toggle scrolling to the bottom. Nothing is
 * carried between presses now (ADR 0147).
 *
 * The numbers are measured, not invented. A seeded six-turn thread in Chromium
 * (crates/lucidos-app e2e, 573px viewport): the answer-only view took the
 * transcript from 2737 to 1915 and the anchored turn's `offsetTop` from 2279 to
 * 1594. The reader sat at 2124, so the correction wanted 1439 against a new
 * maximum of 1342.
 */
describe('a turn control holds its control across a clamp', () => {
  const VIEWPORT = 573;
  const TALL = 2737;
  const SHORT = 1915;
  const ANCHOR_TALL = 2279;
  const ANCHOR_SHORT = 1594;
  /** The reader's offset before the first press, as the measurement had it. */
  const START = 2124;
  /** The most the shrunk transcript can be scrolled to. */
  const SHORT_MAX = SHORT - VIEWPORT; // 1342
  /** How far the pressed control travels between the two views. */
  const DELTA = ANCHOR_TALL - ANCHOR_SHORT; // 685
  /** Where the reverse press lands: its own delta, from where the clamp left
   *  the reader. The 97px the clamp ate is not repaid. */
  const BACK = SHORT_MAX + DELTA; // 2027

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

  /** A pressed control, answering both `offsetTop` and its rect.
   *
   *  The correction measures through rects rather than the platform's
   *  whole-pixel `offsetTop` (see `contentOffsetTop`). Each case MOVES the
   *  control by assigning `offsetTop`, and the rect follows from it. */
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

  /** Where the pressed control sits on screen, which is what a press must hold. */
  function controlTop(anchor: any): number {
    return anchor.getBoundingClientRect().top;
  }

  /** One press: the transcript changes height and the pressed control moves. */
  function press(el: any, anchor: any, nextAnchorTop: number, nextHeight: number) {
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

  it('holds the control on the reveal, after a clamp slid it on the way in', () => {
    const el = makeContainer(START, TALL);
    const anchor: any = makeAnchor(el, ANCHOR_TALL);
    setActiveScrollElement(el);

    press(el, anchor, ANCHOR_SHORT, SHORT);
    // The correction wanted 1439 and the transcript ends at 1342, so the control
    // slid down by the 97 the clamp ate. Nothing can hold it there.
    expect(el.scrollTop).toBe(SHORT_MAX);
    const slidTo = controlTop(anchor);

    press(el, anchor, ANCHOR_TALL, TALL);

    // The reported bug: this press used to spend the clamp and land on START,
    // throwing the control up the screen by 97.
    expect(el.scrollTop).toBe(BACK);
    expect(controlTop(anchor)).toBe(slidTo);
  });

  it('carries nothing between presses, so the pair settles after the clamp', () => {
    const el = makeContainer(START, TALL);
    const anchor: any = makeAnchor(el, ANCHOR_TALL);
    setActiveScrollElement(el);

    press(el, anchor, ANCHOR_SHORT, SHORT);
    press(el, anchor, ANCHOR_TALL, TALL);
    expect(el.scrollTop).toBe(BACK);

    // From BACK the reader has the slack the clamp wanted, so the third press
    // reaches its own target exactly and the fourth returns to it. The drift is
    // one clamp's worth, once, rather than a credit that keeps being spent.
    press(el, anchor, ANCHOR_SHORT, SHORT);
    expect(el.scrollTop).toBe(SHORT_MAX);
    const held = controlTop(anchor);

    press(el, anchor, ANCHOR_TALL, TALL);
    expect(el.scrollTop).toBe(BACK);
    expect(controlTop(anchor)).toBe(held);
  });

  it('moves a press of a DIFFERENT control by its own delta alone', () => {
    // Each press holds the control it was made on. A press higher up the thread
    // reads that control's two offsets, and nothing the last press left behind.
    const el = makeContainer(START, TALL);
    const pressedFirst: any = makeAnchor(el, ANCHOR_TALL);
    const pressedNext: any = makeAnchor(el, 900);
    setActiveScrollElement(el);

    press(el, pressedFirst, ANCHOR_SHORT, SHORT);
    expect(el.scrollTop).toBe(SHORT_MAX);

    press(el, pressedNext, 1200, TALL);
    expect(el.scrollTop).toBe(SHORT_MAX + (1200 - 900));
  });

  it('never moves a transcript too short to scroll', () => {
    const el = makeContainer(0, VIEWPORT);
    const anchor: any = makeAnchor(el, 30);
    setActiveScrollElement(el);

    press(el, anchor, 30, VIEWPORT);
    expect(el.scrollTop).toBe(0);

    press(el, anchor, 30, VIEWPORT);
    expect(el.scrollTop).toBe(0);
  });

  it('keeps no cross-press state in the module', () => {
    // A source scan, because the absence of a mechanism cannot be driven. The
    // correction is one subtraction of two readings taken across one press. A
    // stored credit is what turns it back into two presses talking.
    const source = readFileSync(
      resolve(dirname(fileURLToPath(import.meta.url)), '../CreateThreadView.tsx'),
      'utf8',
    );
    expect(source).toMatch(
      /return reachableScrollTop\(scrollBefore \+ \(offset - offsetBefore\)\);/,
    );
    for (const symbol of ['anchorDebt', 'carriedAnchorDebt', 'rememberAnchorDebt', 'landingScrollTop']) {
      expect(source, `${symbol} is back in CreateThreadView.tsx`).not.toContain(symbol);
    }
  });
});
