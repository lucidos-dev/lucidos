import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

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
import { anchorSpacer } from '../anchorCorrection';
import { setActiveScrollElement, stopFollowingBottom } from '../scrollState';

describe('a turn control holds its control across a clamp', () => {
  const VIEWPORT = 573;
  const TALL = 2737;
  const SHORT = 1915;
  const ANCHOR_TALL = 2279;
  const ANCHOR_SHORT = 1594;
  const START = 2124;
  const SHORT_MAX = SHORT - VIEWPORT;
  const DELTA = ANCHOR_TALL - ANCHOR_SHORT;
  const BACK = SHORT_MAX + DELTA;

  function makeContainer(scrollTop: number, scrollHeight: number) {
    const listeners = new Set<() => void>();
    const el: any = {
      isConnected: true,
      style: mockStyle(),
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
        const max = Math.max(0, v - this.clientHeight);
        if (this._scrollTop > max) this._scrollTop = max;
      },
      getBoundingClientRect: () => ({ width: 800, height: VIEWPORT, top: 0, bottom: VIEWPORT, left: 0, right: 800 }),
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
        const top = container.getBoundingClientRect().top + a.offsetTop + anchorSpacer(container) - container.scrollTop;
        return { width: 800, height: 0, top, bottom: top, left: 0, right: 800 };
      },
    };
    return a as unknown as Element;
  }

  function controlTop(anchor: any): number {
    return anchor.getBoundingClientRect().top;
  }

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
    expect(el.scrollTop).toBe(SHORT_MAX);
    const slidTo = controlTop(anchor);

    press(el, anchor, ANCHOR_TALL, TALL);

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

    press(el, anchor, ANCHOR_SHORT, SHORT);
    expect(el.scrollTop).toBe(SHORT_MAX);
    const held = controlTop(anchor);

    press(el, anchor, ANCHOR_TALL, TALL);
    expect(el.scrollTop).toBe(BACK);
    expect(controlTop(anchor)).toBe(held);
  });

  it('moves a press of a DIFFERENT control by its own delta alone', () => {
    const el = makeContainer(START, TALL);
    const pressedFirst: any = makeAnchor(el, ANCHOR_TALL);
    const pressedNext: any = makeAnchor(el, 900);
    setActiveScrollElement(el);

    press(el, pressedFirst, ANCHOR_SHORT, SHORT);
    expect(el.scrollTop).toBe(SHORT_MAX);

    press(el, pressedNext, 1200, TALL);
    expect(el.scrollTop).toBe(SHORT_MAX + (1200 - 900));
  });

  it('holds a FRACTIONAL move exactly, carrying the rest in the spacer', () => {
    const el = makeContainer(1000, TALL);
    const anchor: any = makeAnchor(el, 1500.25);
    setActiveScrollElement(el);
    const before = controlTop(anchor);

    press(el, anchor, 1200.6, TALL);
    expect(Number.isInteger(el.scrollTop)).toBe(true);
    expect(anchorSpacer(el)).toBeGreaterThan(0);
    expect(anchorSpacer(el)).toBeLessThan(1);
    expect(controlTop(anchor)).toBeCloseTo(before, 9);

    press(el, anchor, 1500.25, TALL);
    expect(el.scrollTop).toBe(1000);
    expect(anchorSpacer(el)).toBe(0);
    expect(controlTop(anchor)).toBeCloseTo(before, 9);
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
    const source = readFileSync(
      resolve(dirname(fileURLToPath(import.meta.url)), '../CreateThreadView.tsx'),
      'utf8',
    );
    expect(source).toMatch(
      /return splitAnchorCorrection\(scrollBefore \+ \(offset - anchorSpacer\(container\) - offsetBefore\)\);/,
    );
    expect(source).toMatch(/if \(settledScrollTop\(container\) !== target\.scrollTop\)/);
    for (const symbol of ['anchorDebt', 'carriedAnchorDebt', 'rememberAnchorDebt', 'landingScrollTop']) {
      expect(source, `${symbol} is back in CreateThreadView.tsx`).not.toContain(symbol);
    }
  });
});
