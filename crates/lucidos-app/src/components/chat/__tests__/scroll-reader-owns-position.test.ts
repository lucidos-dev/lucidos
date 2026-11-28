import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';

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
  classifyScroll,
  isWhereWeLastScrolledIt,
  makeScrollObservers,
  markNavigationScroll,
  scrollToBottom,
  setActiveScrollElement,
  stopFollowingBottom,
} from '../scrollState';


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
    const el = makeEl({ scrollTop: 900, scrollHeight: 3000 });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);

    el.scrollHeight = 3200;
    onResize();
    vi.advanceTimersByTime(2000);

    expect(el.scrollTop).toBe(900);
  });

  it('the same arrival under a reader sitting at the live edge does not move them either', () => {
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
    const el = makeEl({ scrollTop: 0, scrollHeight: 3000, clientHeight: 500 });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);
    atBottom(el);

    el.clientHeight = 460;
    onResize();

    expect(el.scrollTop).toBe(2500);
  });

  it('the iOS keyboard opening and closing leaves the reader still', () => {
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

    expect(el.scrollTop).toBe(2500);
    expect(awayFromBottom.value).toBe(false);
  });

  it('comes BACK as the thread grows, because the tap asked for one jump', () => {
    const el = makeEl({ scrollTop: 100, scrollHeight: 3000 });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);

    scrollToBottom();
    expect(awayFromBottom.value).toBe(false);
    const landed = el.scrollTop;

    el.scrollHeight = 3400;
    onResize();

    expect(awayFromBottom.value).toBe(true);
    expect(el.scrollTop).toBe(landed);
  });

  it('reconciles the chevron even when the container was already at the bottom', () => {
    const el = makeEl({ scrollTop: 0, scrollHeight: 3000 });
    setActiveScrollElement(el);
    atBottom(el);
    awayFromBottom.value = true;

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

    el.scrollTop = 2497;
    onScroll();
    expect(awayFromBottom.value).toBe(true);

    atBottom(el);
    onScroll();
    expect(awayFromBottom.value).toBe(false);
  });
});

describe('a navigation owns the scroll event it is about to fire', () => {
  beforeEach(() => { stopFollowingBottom(); });
  afterEach(() => { setActiveScrollElement(null); });

  it('claims the event right after an instant jump, then lapses on its own', async () => {
    const el = makeEl({ scrollTop: 100, scrollHeight: 3000 });
    setActiveScrollElement(el);
    await new Promise(r => setTimeout(r, 100));
    expect(classifyScroll().navigation).toBe(false);

    scrollToBottom();
    expect(classifyScroll().navigation).toBe(true);

    await new Promise(r => setTimeout(r, 100));
    expect(classifyScroll().navigation).toBe(false);
  });
});

describe('a navigation still owns its scroll event when the event comes late', () => {
  it('reads a late event on the position it wrote as ours', async () => {
    const el = makeEl({ scrollTop: 0, scrollHeight: 8000 });
    markNavigationScroll(el, 818);
    await new Promise(r => setTimeout(r, 100));
    expect(classifyScroll(el).navigation).toBe(false);
    expect(isWhereWeLastScrolledIt(el)).toBe(true);
  });

  it('reads the repaint nudge a pixel either side as ours too', () => {
    const el = makeEl({ scrollTop: 0, scrollHeight: 8000 });
    markNavigationScroll(el, 818);
    el.scrollTop = 817;
    expect(isWhereWeLastScrolledIt(el)).toBe(true);
    el.scrollTop = 819;
    expect(isWhereWeLastScrolledIt(el)).toBe(true);
  });

  it('lets the reader go the moment they move', () => {
    const el = makeEl({ scrollTop: 0, scrollHeight: 8000 });
    markNavigationScroll(el, 818);
    el.scrollTop = 700;
    expect(isWhereWeLastScrolledIt(el)).toBe(false);
  });

  it('records where the browser settled a clamped write', () => {
    const el = makeEl({ scrollTop: 0, scrollHeight: 1000, clientHeight: 500 });
    markNavigationScroll(el, 5000);
    expect(el.scrollTop).toBe(500);
    expect(isWhereWeLastScrolledIt(el)).toBe(true);
  });

  it('retires once a scroll finds the container anywhere else', () => {
    const el = makeEl({ scrollTop: 0, scrollHeight: 8000 });
    const { onScroll } = makeScrollObservers(el);
    markNavigationScroll(el, 0);
    el.scrollTop = 3000;
    onScroll();
    el.scrollTop = 0;
    expect(isWhereWeLastScrolledIt(el)).toBe(false);
  });

  it('holds through its own event and the repaint nudge', () => {
    const el = makeEl({ scrollTop: 0, scrollHeight: 8000 });
    const { onScroll } = makeScrollObservers(el);
    markNavigationScroll(el, 818);
    onScroll();
    el.scrollTop = 817;
    onScroll();
    el.scrollTop = 818;
    onScroll();
    expect(isWhereWeLastScrolledIt(el)).toBe(true);
  });

  it('never speaks for another container', () => {
    const written = makeEl({ scrollTop: 0, scrollHeight: 8000 });
    const other = makeEl({ scrollTop: 818, scrollHeight: 8000 });
    markNavigationScroll(written, 818);
    expect(isWhereWeLastScrolledIt(other)).toBe(false);
  });
});

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
    const hiddenDesktop = { scrollTop: 0, scrollHeight: 2000 };
    const visibleMobile = mockScrollEl({ scrollTop: 0, scrollHeight: 3000 });

    setActiveScrollElement(visibleMobile);

    scrollToBottom();

    expect(visibleMobile.scrollTop).toBe(3000);
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

    scrollToBottom();

    expect(fallbackEl.scrollTop).toBe(1500);

    document.querySelectorAll = origQSA;
  });

  it('switching threads updates the active element', () => {
    const threadA = mockScrollEl({ scrollTop: 0, scrollHeight: 1000 });
    const threadB = mockScrollEl({ scrollTop: 0, scrollHeight: 2000 });

    setActiveScrollElement(threadA);
    scrollToBottom();
    expect(threadA.scrollTop).toBe(1000);

    setActiveScrollElement(threadB);
    scrollToBottom();
    expect(threadB.scrollTop).toBe(2000);
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
