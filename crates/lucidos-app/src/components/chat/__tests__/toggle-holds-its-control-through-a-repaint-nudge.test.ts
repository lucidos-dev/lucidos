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

let nudged: { el: any; shift: number } | null = null;

function nudgeShift(el: any): number {
  return nudged !== null && nudged.el === el ? nudged.shift : 0;
}

vi.mock('../../../utils/webkitRepaint', () => ({
  forceWebKitRepaint: () => undefined,
  settledScrollTop: (el: any) => el.scrollTop - nudgeShift(el),
}));

const { withScrollAnchor } = await import('../CreateThreadView');
const { mockStyle } = await import('./scroll-test-helpers');
const { anchorSpacer } = await import('../anchorCorrection');
const { setActiveScrollElement, stopFollowingBottom } = await import('../scrollState');

describe('a turn control pressed during a repaint nudge', () => {
  const VIEWPORT = 600;
  const HEIGHT = 4000;

  function makeContainer(scrollTop: number) {
    const el: any = {
      isConnected: true,
      style: mockStyle(),
      clientHeight: VIEWPORT,
      clientWidth: 800,
      offsetHeight: VIEWPORT,
      scrollHeight: HEIGHT,
      children: [],
      _scrollTop: scrollTop,
      addEventListener() {},
      removeEventListener() {},
      get scrollTop() { return this._scrollTop; },
      set scrollTop(v: number) {
        this._scrollTop = Math.min(Math.max(0, v), this.scrollHeight - this.clientHeight);
        if (nudged?.el === this) nudged = null;
      },
      getBoundingClientRect() {
        const top = nudgeShift(el);
        return { width: 800, height: VIEWPORT, top, bottom: top + VIEWPORT, left: 0, right: 800 };
      },
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
    return a;
  }

  function nudge(el: any) {
    el._scrollTop -= 1;
    nudged = { el, shift: -1 };
  }

  beforeEach(() => {
    nudged = null;
    stopFollowingBottom();
    setActiveScrollElement(null);
    vi.useFakeTimers();
  });
  afterEach(() => {
    vi.clearAllTimers();
    vi.useRealTimers();
    stopFollowingBottom();
    setActiveScrollElement(null);
    nudged = null;
  });

  it('measures the reader from the settled offset, not the nudged one', () => {
    const el = makeContainer(1000);
    const anchor = makeAnchor(el, 1500);
    setActiveScrollElement(el);
    nudge(el);
    const seen = anchor.getBoundingClientRect().top;
    expect(seen).toBe(500);

    withScrollAnchor(anchor, () => { anchor.offsetTop = 1200; });
    vi.advanceTimersByTime(1500);

    expect(el.scrollTop).toBe(700);
    expect(anchor.getBoundingClientRect().top).toBe(seen);
  });
});
