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
import { anchorSpacer } from '../anchorCorrection';
import { readerGestureForTest, setActiveScrollElement, stopFollowingBottom } from '../scrollState';

describe('a turn control holds its control through a late render', () => {
  const VIEWPORT = 573;
  const TALL = 6500;
  const SHORT = 4581;
  const ANCHOR_TALL = 4860;
  const ANCHOR_SHORT = 2941;
  const START = 4820;
  const HELD = START - (ANCHOR_TALL - ANCHOR_SHORT);
  const RENDER_AT = 10;
  const FREEZE_REFLOW = 9;

  function makeContainer() {
    const el: any = {
      isConnected: true,
      parentElement: null,
      children: [],
      style: mockStyle({ overflow: '', transform: '' }),
      clientWidth: 800,
      clientHeight: VIEWPORT,
      offsetHeight: VIEWPORT,
      settled: [] as number[],
      _scrollTop: START,
      _scrollHeight: TALL,
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
      getBoundingClientRect: () => ({ width: 800, height: VIEWPORT, top: 0, bottom: VIEWPORT, left: 0, right: 800 }),
      querySelectorAll: () => [],
    };
    return el;
  }

  function makeAnchor(container: any) {
    const a: any = {
      isConnected: true,
      offsetTop: ANCHOR_TALL,
      closest: (sel: string) => (sel === '.thread-content' ? container : null),
      getBoundingClientRect: () => {
        const reflow = container.style.overflow === 'hidden' ? FREEZE_REFLOW : 0;
        const top = a.offsetTop + reflow + anchorSpacer(container) - container.scrollTop;
        return { width: 800, height: 0, top, bottom: top, left: 0, right: 800 };
      },
    };
    return a;
  }

  function render(el: any, anchor: any) {
    anchor.offsetTop = ANCHOR_SHORT;
    el.scrollHeight = SHORT;
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
    readerGestureForTest(null);
  });

  it('holds the control for a reader who scrolled to it just before pressing', async () => {
    const el = makeContainer();
    const anchor = makeAnchor(el);
    setActiveScrollElement(el);
    readerGestureForTest(el);
    await vi.advanceTimersByTimeAsync(200);
    const before = anchor.getBoundingClientRect().top;

    withScrollAnchor(anchor, () => {
      setTimeout(() => render(el, anchor), RENDER_AT);
    });
    await vi.advanceTimersByTimeAsync(1500);

    expect(el.scrollTop).toBe(HELD);
    expect(anchor.getBoundingClientRect().top).toBe(before);
  });

  it('stands down for a gesture the reader makes after the press', async () => {
    const el = makeContainer();
    const anchor = makeAnchor(el);
    setActiveScrollElement(el);

    withScrollAnchor(anchor, () => {
      setTimeout(() => render(el, anchor), RENDER_AT);
    });
    await vi.advanceTimersByTimeAsync(RENDER_AT + 2);
    readerGestureForTest(el);
    await vi.advanceTimersByTimeAsync(1500);

    expect(el.scrollTop).toBe(SHORT - VIEWPORT);
  });

  it('holds the control when the render commits after the correction', async () => {
    const el = makeContainer();
    const anchor = makeAnchor(el);
    setActiveScrollElement(el);
    const before = anchor.getBoundingClientRect().top;

    withScrollAnchor(anchor, () => {
      setTimeout(() => render(el, anchor), RENDER_AT);
    });
    await vi.advanceTimersByTimeAsync(1500);

    expect(el.scrollTop).not.toBe(SHORT - VIEWPORT);
    expect(el.scrollTop).toBe(HELD);
    expect(anchor.getBoundingClientRect().top).toBe(before);
  });

  it('measures the correction in the layout the reader sees, not the frozen one', () => {
    const el = makeContainer();
    const anchor = makeAnchor(el);
    setActiveScrollElement(el);
    const before = anchor.getBoundingClientRect().top;

    withScrollAnchor(anchor, () => render(el, anchor));
    vi.advanceTimersByTime(1500);

    expect(el.settled).toEqual([SHORT - VIEWPORT, HELD]);
    expect(anchor.getBoundingClientRect().top).toBe(before);
  });
});
