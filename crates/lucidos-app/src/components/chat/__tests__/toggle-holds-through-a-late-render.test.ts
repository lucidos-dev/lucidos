import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';

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
import { mockStyle } from './scroll-test-helpers';
import { anchorSpacer } from '../anchorCorrection';
import { readerGestureForTest, setActiveScrollElement, stopFollowingBottom } from '../scrollState';

/**
 * **A turn control holds its control when the render lands after the correction.**
 *
 * Reported on the desktop app: "Show the latest answer only", pressed on the
 * last turn, sent the reader to the bottom of the thread. That turn did not
 * change at all, and every turn above it shrank.
 *
 * Two things lined up, both WebKit's:
 *
 * - The freeze itself moves the control. WebKit drops the scrollbar gutter
 *   under `overflow: hidden`, which rewraps every line, so the synchronous
 *   check read a change and corrected before the render had happened.
 * - The render then shrank the transcript under the reader, and the browser
 *   clamped the offset to the bottom. The re-assert read that clamp as the
 *   reader scrolling away, and stood down.
 *
 * The numbers are the reported thread's, measured in Playwright's WebKit.
 */
describe('a turn control holds its control through a late render', () => {
  const VIEWPORT = 573;
  const TALL = 6500;
  const SHORT = 4581;
  const ANCHOR_TALL = 4860;
  const ANCHOR_SHORT = 2941;
  const START = 4820;
  /** Where holding the control puts the reader: the whole shrink was above it. */
  const HELD = START - (ANCHOR_TALL - ANCHOR_SHORT); // 2901
  /** When the render commits: after the press's own correction has run, and
   *  before the first re-assert frame. The async timer advance flushes
   *  microtasks, so the correction runs where a browser runs it. */
  const RENDER_AT = 10;
  /** How far the freeze's gutter reflow moves the control, as measured. */
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
      /** Every position the container was left at, in order. */
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

  /** The pressed control. While the container is frozen it reads the gutter
   *  reflow on top of its real offset, as WebKit's layout does. */
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

  /** The render the press asks for: every turn above the control shrinks. */
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
    // The ordinary way to reach a control. That scroll is still inside the
    // reader-gesture window when the press lands, and must not end its re-assert.
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

    // Nothing wrote past the reader: the offset is where the render's clamp left it.
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

    // The reported bug left the reader on the clamp, at the bottom.
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

    // The render's own clamp, then the correction, and nothing between. A
    // correction read in the frozen layout lands 9px off for a frame first.
    expect(el.settled).toEqual([SHORT - VIEWPORT, HELD]);
    expect(anchor.getBoundingClientRect().top).toBe(before);
  });
});
