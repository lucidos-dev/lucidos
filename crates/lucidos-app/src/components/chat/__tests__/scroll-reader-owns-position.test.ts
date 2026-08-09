import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';

// Stub HTMLElement before importing modules that reference it
if (typeof globalThis.HTMLElement === 'undefined') {
  (globalThis as any).HTMLElement = class HTMLElement {};
}
if (typeof globalThis.document !== 'undefined' && !('activeElement' in globalThis.document)) {
  (globalThis.document as any).activeElement = null;
}
if (typeof globalThis.requestAnimationFrame === 'undefined') {
  (globalThis as any).requestAnimationFrame = (cb: any) => { cb(); return 0; };
}
if (typeof globalThis.cancelAnimationFrame === 'undefined') {
  (globalThis as any).cancelAnimationFrame = () => {};
}
if (typeof globalThis.queueMicrotask === 'undefined') {
  (globalThis as any).queueMicrotask = (cb: any) => { Promise.resolve().then(cb); };
}

import { mockScrollEl } from './scroll-test-helpers';
import {
  awayFromBottom,
  getActiveScrollElement,
  isNavigationScroll,
  makeScrollObservers,
  scrollToBottom,
  setActiveScrollElement,
  stopFollowingBottom,
} from '../scrollState';

/** The transcript's scroll position belongs to the reader.
 *
 *  This file was `scroll-suppression-preserve.test.ts` and asserted the opposite:
 *  that a send engaged a 500ms suppression window, that a 16ms loop re-pinned
 *  the container to the bottom through it, that `preserveAtBottom` re-armed the
 *  window on every keystroke, and that `pinToBottomNow` re-pinned across an iOS
 *  keyboard resize. Every one of those cases is kept below, inverted: the same
 *  event happens, and the reader does not move.
 *
 *  Growth is the subject here, never a request. The two things a reader CAN ask
 *  for, the down chevron and a send, arm a standing follow that growth honours,
 *  and they live in `scroll-follow-the-live-edge.test.ts`. Nothing below arms
 *  anything, which is exactly why the same growth moves nobody.
 *
 *  The old suite mirrored `onResize` with a hand-rolled copy in the test file,
 *  so it could pass while the real handler did something else. These drive the
 *  REAL `makeScrollObservers` instead. */

/** A `.thread-content` stand-in that clamps `scrollTop` the way a browser does,
 *  so an out-of-range write cannot park a number that hides an off-bottom state.
 *  `parentElement: null` and a non-zero rect keep `isElementVisible` happy. */
function makeEl(opts: { scrollTop: number; scrollHeight: number; clientHeight?: number }) {
  const el: any = {
    parentElement: null,
    children: [],
    clientWidth: 800,
    clientHeight: opts.clientHeight ?? 500,
    scrollHeight: opts.scrollHeight,
    _scrollTop: opts.scrollTop,
    get scrollTop() { return this._scrollTop; },
    set scrollTop(v: number) {
      this._scrollTop = Math.min(Math.max(0, v), Math.max(0, this.scrollHeight - this.clientHeight));
    },
    getBoundingClientRect: () => ({ width: 800, height: el.clientHeight, top: 0, bottom: el.clientHeight, left: 0, right: 800 }),
  };
  return el;
}

/** Park the reader exactly at the live edge of `el`. */
function atBottom(el: any) {
  el.scrollTop = el.scrollHeight;
  return el.scrollTop;
}

describe('nothing the app does moves the transcript', () => {
  beforeEach(() => {
    stopFollowingBottom();
    awayFromBottom.value = false;
    vi.useFakeTimers();
  });
  afterEach(() => {
    vi.clearAllTimers();
    vi.useRealTimers();
    setActiveScrollElement(null);
  });

  it('a message arriving from somewhere else leaves the viewport exactly where it was', () => {
    // A peer device's send, or an injected prompt: a user row appears at the
    // bottom that THIS reader did not ask for. It used to trigger the pin from
    // three places at once (PromptInput.submit, addPendingMessage, ThreadView's
    // pending-count effect); now it simply renders below them. This reader's own
    // send is a different case and arms the follow: see
    // scroll-follow-the-live-edge.test.ts.
    const el = makeEl({ scrollTop: 900, scrollHeight: 3000 });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);

    el.scrollHeight = 3200; // the user row renders
    onResize();
    vi.advanceTimersByTime(2000); // and no loop is running behind it

    expect(el.scrollTop).toBe(900);
  });

  it('the same arrival under a reader sitting at the live edge does not move them either', () => {
    // The case the pin existed for. Sitting at the bottom is a position, not a
    // request, so the row lands just below the fold and the chevron is how they
    // choose to follow it.
    const el = makeEl({ scrollTop: 0, scrollHeight: 3000 });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);
    const wasAt = atBottom(el);

    el.scrollHeight = 3200;
    onResize();

    expect(el.scrollTop).toBe(wasAt);
    expect(awayFromBottom.value).toBe(true);
  });

  it('a streaming reply does not drag the viewport, however many chunks land', () => {
    const el = makeEl({ scrollTop: 2500, scrollHeight: 3000 });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);

    for (const height of [3200, 3600, 4400, 9000]) {
      el.scrollHeight = height;
      onResize();
      expect(el.scrollTop).toBe(2500);
    }
    expect(awayFromBottom.value).toBe(true);
  });

  it('the composer growing under a reader at the live edge does not move them', () => {
    // Typing a first character swaps the action row and can take height from the
    // transcript, shrinking its clientHeight. This used to call
    // `preserveAtBottom()` on EVERY keystroke, which armed the resize force-pin.
    const el = makeEl({ scrollTop: 0, scrollHeight: 3000, clientHeight: 500 });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);
    atBottom(el); // 2500

    el.clientHeight = 460; // the composer took 40px
    onResize();

    expect(el.scrollTop).toBe(2500);
  });

  it('the iOS keyboard opening and closing leaves the reader still', () => {
    // visualViewport resizes fire repeatedly through the ~350ms keyboard
    // animation. MobileSwipeContainer used to re-pin on each of them.
    const el = makeEl({ scrollTop: 1200, scrollHeight: 4000, clientHeight: 600 });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);

    for (const clientHeight of [550, 480, 420, 350, 300, 350, 480, 600]) {
      el.clientHeight = clientHeight;
      onResize();
      expect(el.scrollTop).toBe(1200);
    }
  });

  it('no timer keeps writing scrollTop after an explicit go-to-bottom', () => {
    // The 16ms re-pinning loop is gone: one write, then silence. A loop left
    // running is what cancelled iOS momentum mid-fling, and what needed a frame
    // budget as a backstop against a lost suppression timer.
    let writes = 0;
    const el: any = {
      parentElement: null,
      children: [],
      clientWidth: 800,
      clientHeight: 500,
      scrollHeight: 2000,
      _scrollTop: 100,
      get scrollTop() { return this._scrollTop; },
      set scrollTop(v: number) { this._scrollTop = v; writes++; },
      getBoundingClientRect: () => ({ width: 800, height: 500, top: 0, bottom: 500, left: 0, right: 800 }),
    };
    setActiveScrollElement(el);

    scrollToBottom();
    expect(writes).toBe(1);

    vi.advanceTimersByTime(10_000);
    expect(writes).toBe(1);
  });
});

describe('the chevron is the way back to the live edge', () => {
  beforeEach(() => {
    stopFollowingBottom();
    awayFromBottom.value = true;
    vi.useFakeTimers();
  });
  afterEach(() => {
    vi.clearAllTimers();
    vi.useRealTimers();
    setActiveScrollElement(null);
  });

  it('goes to the bottom from anywhere, and hides itself on arrival', () => {
    const el = makeEl({ scrollTop: 100, scrollHeight: 3000 });
    setActiveScrollElement(el);

    scrollToBottom();

    expect(el.scrollTop).toBe(2500); // 3000 - 500
    expect(awayFromBottom.value).toBe(false);
  });

  it('stays hidden as the thread grows, because the tap asked to ride the live edge', () => {
    // The chevron is not a one-shot jump: it arms the standing follow, so the
    // next chunk carries the reader with it and there is nothing to come back
    // for. The chevron reappearing on growth is the UNARMED reader's case, one
    // describe above.
    const el = makeEl({ scrollTop: 100, scrollHeight: 3000 });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);

    scrollToBottom();
    expect(awayFromBottom.value).toBe(false);

    el.scrollHeight = 3400;
    onResize();

    expect(awayFromBottom.value).toBe(false);
    expect(el.scrollTop).toBe(2900); // carried to the new live edge
  });

  it('reconciles the chevron even when the container was already at the bottom', () => {
    // The write is a no-op there, so no scroll event fires and nothing else
    // would settle the signal.
    const el = makeEl({ scrollTop: 0, scrollHeight: 3000 });
    setActiveScrollElement(el);
    atBottom(el);
    awayFromBottom.value = true; // stale

    scrollToBottom();

    expect(awayFromBottom.value).toBe(false);
  });
});

describe('onScroll reconciles the chevron in both directions', () => {
  beforeEach(() => { awayFromBottom.value = false; });
  afterEach(() => { setActiveScrollElement(null); });

  it('raises it on the first pixel off the bottom and clears it on return', () => {
    const el = makeEl({ scrollTop: 0, scrollHeight: 3000 });
    const { onScroll } = makeScrollObservers(el);
    atBottom(el);
    onScroll();
    expect(awayFromBottom.value).toBe(false);

    el.scrollTop = 2497; // three pixels up, past the 2px subpixel slack
    onScroll();
    expect(awayFromBottom.value).toBe(true);

    atBottom(el);
    onScroll();
    expect(awayFromBottom.value).toBe(false);
  });
});

// ---------------------------------------------------------------------------
// A navigation owns the scroll event it is ABOUT to fire.
//
// A scrollTop write does not dispatch its scroll event synchronously: the
// browser fires it at the next rendering opportunity. So the two consumers that
// must tell our scrolls from the reader's (the mobile hide-on-scroll header and
// the mobile scroll indicator) ask AFTER the write has returned, and an "is a
// tween running" test alone answers false for exactly the events it exists to
// catch: every instant navigation runs no tween at all, and a tween clears its
// rAF handle on the frame it lands. The header would hide on a chevron tap,
// which is the case the retired resize-mode window covered by accident.
//
// Real timers, deliberately: the window is measured with `performance.now()`,
// which vitest does not fake, so `advanceTimersByTime` cannot move it.
// ---------------------------------------------------------------------------
describe('a navigation owns the scroll event it is about to fire', () => {
  beforeEach(() => { stopFollowingBottom(); });
  afterEach(() => { setActiveScrollElement(null); });

  it('claims the event right after an instant jump, then lapses on its own', async () => {
    const el = makeEl({ scrollTop: 100, scrollHeight: 3000 });
    setActiveScrollElement(el);
    // Clear any window a preceding test left open (the stamp is module state).
    await new Promise(r => setTimeout(r, 100));
    expect(isNavigationScroll()).toBe(false);

    scrollToBottom(); // no tween runs on this path at all
    expect(isNavigationScroll()).toBe(true);

    await new Promise(r => setTimeout(r, 100)); // past NAV_SCROLL_EVENT_WINDOW_MS
    expect(isNavigationScroll()).toBe(false);
  });
});

// ---------------------------------------------------------------------------
// Active scroll element: an explicit go-to-bottom must use the registered
// element, not querySelector (which finds the wrong element on mobile or
// during component transitions).
// ---------------------------------------------------------------------------
describe('active scroll element registration', () => {
  beforeEach(() => {
    stopFollowingBottom();
    setActiveScrollElement(null);
    vi.useFakeTimers();
  });
  afterEach(() => {
    setActiveScrollElement(null);
    vi.useRealTimers();
  });

  it('scrollToBottom uses registered active element, not querySelector', () => {
    // Simulate mobile: querySelectorAll would find the hidden desktop element first,
    // but the active element is the mobile one.
    const hiddenDesktop = { scrollTop: 0, scrollHeight: 2000 };
    const visibleMobile = mockScrollEl({ scrollTop: 0, scrollHeight: 3000 });

    // Register the mobile element as active
    setActiveScrollElement(visibleMobile);

    scrollToBottom();

    // Must scroll the mobile element, NOT the hidden desktop one
    expect(visibleMobile.scrollTop).toBe(3000);
    // Desktop element should NOT have been scrolled
    expect(hiddenDesktop.scrollTop).toBe(0);
  });

  it('scrollToBottom falls back to querySelectorAll when no active element', () => {
    const fallbackEl = {
      scrollTop: 0,
      scrollHeight: 1500,
      getBoundingClientRect: () => ({ width: 400, height: 600 }),
      scrollTo(arg: any) { if (typeof arg === 'object') fallbackEl.scrollTop = arg.top; },
    };
    const origQSA = document.querySelectorAll;
    document.querySelectorAll = vi.fn(() => [fallbackEl] as any);

    // No active element registered
    scrollToBottom();

    expect(fallbackEl.scrollTop).toBe(1500);

    document.querySelectorAll = origQSA;
  });

  it('switching threads updates the active element', () => {
    const threadA = mockScrollEl({ scrollTop: 0, scrollHeight: 1000 });
    const threadB = mockScrollEl({ scrollTop: 0, scrollHeight: 2000 });

    // Thread A is active
    setActiveScrollElement(threadA);
    scrollToBottom();
    expect(threadA.scrollTop).toBe(1000);

    // Switch to thread B
    setActiveScrollElement(threadB);
    scrollToBottom();
    expect(threadB.scrollTop).toBe(2000);
    // Thread A should not be scrolled again
    expect(threadA.scrollTop).toBe(1000);
  });

  it('getActiveScrollElement returns the registered element', () => {
    const el = { scrollTop: 0, scrollHeight: 500 } as any;
    expect(getActiveScrollElement()).toBeNull();
    setActiveScrollElement(el);
    expect(getActiveScrollElement()).toBe(el);
    setActiveScrollElement(null);
    expect(getActiveScrollElement()).toBeNull();
  });
});
