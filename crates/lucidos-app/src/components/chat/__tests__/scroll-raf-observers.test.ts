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
import { composeHandlers } from '../composeHandlers';
import { awayFromBottom, makeScrollObservers, notAtTop, scrollToBottom, setActiveScrollElement, stopFollowingBottom } from '../scrollState';

describe('scrollToBottom is one write, not a tail', () => {
  beforeEach(() => {
    stopFollowingBottom();
    awayFromBottom.value = false;
    vi.useFakeTimers();
  });
  afterEach(() => {
    setActiveScrollElement(null);
    vi.clearAllTimers();
    vi.useRealTimers();
  });

  it('goes to the bottom as it is NOW, with no timer dragging the reader after it', () => {
    const el = mockScrollEl({ scrollTop: 0, scrollHeight: 2000 });
    setActiveScrollElement(el);

    scrollToBottom();
    expect(el.scrollTop).toBe(2000);

    for (const height of [2100, 2300, 2500]) {
      el.scrollHeight = height;
      vi.advanceTimersByTime(100);
      expect(el.scrollTop).toBe(2000);
    }
  });

  it('resolves the target at call time, so a later element swap is not chased', () => {
    const elA = mockScrollEl({ scrollTop: 0, scrollHeight: 1000 });
    const elB = mockScrollEl({ scrollTop: 0, scrollHeight: 2000 });

    setActiveScrollElement(elA);
    scrollToBottom();
    expect(elA.scrollTop).toBe(1000);

    setActiveScrollElement(elB);
    vi.advanceTimersByTime(500);
    expect(elB.scrollTop).toBe(0);

    scrollToBottom();
    expect(elB.scrollTop).toBe(2000);
  });

  it('a second tap goes to the newer bottom', () => {
    const el = mockScrollEl({ scrollTop: 0, scrollHeight: 1000 });
    setActiveScrollElement(el);

    scrollToBottom();
    expect(el.scrollTop).toBe(1000);

    el.scrollHeight = 2000;
    scrollToBottom();
    expect(el.scrollTop).toBe(2000);
  });
});

describe('scrollToBottom does not touch notAtTop (scroll listener owns it)', () => {
  beforeEach(() => {
    stopFollowingBottom();
    notAtTop.value = false;
    vi.useFakeTimers();
  });
  afterEach(() => {
    setActiveScrollElement(null);
    vi.clearAllTimers();
    vi.useRealTimers();
  });

  it('does not set notAtTop even when scrolled far from top', () => {
    const el = mockScrollEl({ scrollTop: 0, scrollHeight: 2000 });
    setActiveScrollElement(el);
    notAtTop.value = false;

    scrollToBottom();

    expect(el.scrollTop).toBe(2000);
    expect(notAtTop.value).toBe(false);
  });

  it('does not clear notAtTop when content fits in viewport', () => {
    const el = mockScrollEl({ scrollTop: 0, scrollHeight: 50 });
    setActiveScrollElement(el);
    notAtTop.value = true;

    scrollToBottom();

    expect(notAtTop.value).toBe(true);
  });
});

describe('composeHandlers focuses before action', () => {
  it('composeHandlers focuses prompt before action', () => {
    const order: string[] = [];
    const focusFn = () => order.push('focus');
    const action = () => order.push('action');

    const handlers = composeHandlers(action, focusFn);

    const touchEvent = { preventDefault: vi.fn() } as any;
    handlers.onTouchEnd(touchEvent);

    expect(order).toEqual(['focus', 'action']);
    expect(touchEvent.preventDefault).toHaveBeenCalled();
  });

  it('composeHandlers onClick path also focuses first', () => {
    const order: string[] = [];
    const focusFn = () => order.push('focus');
    const action = () => order.push('action');

    const handlers = composeHandlers(action, focusFn);

    handlers.onClick();
    expect(order).toEqual(['focus', 'action']);
  });

  it('touchend prevents subsequent click from double-firing', () => {
    let actionCount = 0;
    const focusFn = () => {};
    const action = () => actionCount++;

    const handlers = composeHandlers(action, focusFn);

    handlers.onTouchEnd({ preventDefault: vi.fn() } as any);
    handlers.onClick();

    expect(actionCount).toBe(1);
  });
});

describe('response-panel collapse → expand re-shows the scroll-to-bottom chevron', () => {
  function makeClampingEl(opts: {
    scrollTop: number;
    scrollHeight: number;
    clientHeight: number;
  }) {
    const listeners: Record<string, Array<() => void>> = {};
    let _scrollTop = opts.scrollTop;
    let _scrollHeight = opts.scrollHeight;
    const el: any = {
      get scrollTop() { return _scrollTop; },
      set scrollTop(v: number) {
        const max = Math.max(0, _scrollHeight - opts.clientHeight);
        const clamped = Math.max(0, Math.min(v, max));
        if (clamped === _scrollTop) return;
        _scrollTop = clamped;
        for (const cb of listeners.scroll ?? []) cb();
      },
      get scrollHeight() { return _scrollHeight; },
      set scrollHeight(v: number) {
        _scrollHeight = v;
        const max = Math.max(0, _scrollHeight - opts.clientHeight);
        if (_scrollTop > max) {
          _scrollTop = max;
          for (const cb of listeners.scroll ?? []) cb();
        }
      },
      clientHeight: opts.clientHeight,
      getBoundingClientRect: () => ({ width: 400, height: opts.clientHeight }),
      parentElement: null,
      addEventListener: (type: string, cb: () => void) => {
        (listeners[type] ??= []).push(cb);
      },
      removeEventListener: (type: string, cb: () => void) => {
        listeners[type] = (listeners[type] ?? []).filter(f => f !== cb);
      },
    };
    return el;
  }

  beforeEach(() => {
    stopFollowingBottom();
    awayFromBottom.value = false;
    notAtTop.value = false;
    setActiveScrollElement(null);
  });
  afterEach(() => { setActiveScrollElement(null); });

  it('the toggle shows the chevron and holds the reader still', () => {
    const el = makeClampingEl({ scrollTop: 1000, scrollHeight: 1500, clientHeight: 500 });
    const { onScroll, onResize } = makeScrollObservers(el);
    el.addEventListener('scroll', onScroll);

    el.scrollHeight = 700;
    onResize();
    expect(awayFromBottom.value).toBe(false);
    const afterCollapse = el.scrollTop;

    el.scrollHeight = 1500;
    onResize();

    expect(awayFromBottom.value).toBe(true);
    expect(el.scrollTop).toBe(afterCollapse);
    expect(el.scrollTop).toBeLessThan(el.scrollHeight - el.clientHeight);
  });

  it('an expand during streaming still shows the chevron, with nothing racing it', () => {
    const el = makeClampingEl({ scrollTop: 1000, scrollHeight: 1500, clientHeight: 500 });
    setActiveScrollElement(el);
    const { onScroll, onResize } = makeScrollObservers(el);
    el.addEventListener('scroll', onScroll);

    el.scrollHeight = 700;
    onResize();
    const afterCollapse = el.scrollTop;

    el.scrollHeight = 1500;
    onResize();
    el.scrollHeight = 1900;
    onResize();

    expect(awayFromBottom.value).toBe(true);
    expect(el.scrollTop).toBe(afterCollapse);
  });

  it('a collapse that leaves the reader at the new bottom hides the chevron', () => {
    const el = makeClampingEl({ scrollTop: 1000, scrollHeight: 1500, clientHeight: 500 });
    setActiveScrollElement(el);
    const { onScroll, onResize } = makeScrollObservers(el);
    el.addEventListener('scroll', onScroll);
    awayFromBottom.value = true;

    el.scrollHeight = 700;
    onResize();

    expect(awayFromBottom.value).toBe(false);
  });
});

describe('makeScrollObservers — hidden duplicate must not override signals', () => {
  function makeEl(opts: {
    visible: boolean;
    scrollTop?: number;
    scrollHeight?: number;
    clientHeight?: number;
  }) {
    const el: any = {
      scrollTop: opts.scrollTop ?? 0,
      scrollHeight: opts.scrollHeight ?? 0,
      clientHeight: opts.clientHeight ?? 0,
      getBoundingClientRect: () => opts.visible
        ? { width: 400, height: 600 }
        : { width: 0, height: 0 },
      parentElement: null,
    };
    return el;
  }

  beforeEach(() => {
    stopFollowingBottom();
    notAtTop.value = false;
    awayFromBottom.value = false;
  });

  it('hidden el onResize does not clear notAtTop set by visible el', () => {
    const visible = makeEl({ visible: true, scrollTop: 4500, scrollHeight: 5200, clientHeight: 700 });
    const hidden = makeEl({ visible: false });

    const visibleObs = makeScrollObservers(visible);
    const hiddenObs = makeScrollObservers(hidden);

    visibleObs.onResize();
    expect(notAtTop.value).toBe(true);

    hiddenObs.onResize();
    expect(notAtTop.value).toBe(true);
  });

  it('hidden el onScroll does not clear awayFromBottom set by visible el', () => {
    const visible = makeEl({ visible: true, scrollTop: 100, scrollHeight: 5000, clientHeight: 700 });
    const hidden = makeEl({ visible: false });

    const visibleObs = makeScrollObservers(visible);
    const hiddenObs = makeScrollObservers(hidden);

    visibleObs.onScroll();
    expect(awayFromBottom.value).toBe(true);

    hiddenObs.onScroll();
    expect(awayFromBottom.value).toBe(true);
  });

  it('visible el onResize sets notAtTop when scrolled away from top', () => {
    const visible = makeEl({ visible: true, scrollTop: 4500, scrollHeight: 5200, clientHeight: 700 });
    const visibleObs = makeScrollObservers(visible);

    visibleObs.onResize();

    expect(notAtTop.value).toBe(true);
  });

  it('visible el onResize clears notAtTop when at top', () => {
    notAtTop.value = true;
    const visible = makeEl({ visible: true, scrollTop: 0, scrollHeight: 5200, clientHeight: 700 });
    const visibleObs = makeScrollObservers(visible);

    visibleObs.onResize();

    expect(notAtTop.value).toBe(false);
  });
});

describe('tab visibility does not move the transcript', () => {
  function setVisibility(state: 'hidden' | 'visible') {
    Object.defineProperty(document, 'visibilityState', { configurable: true, value: state });
    document.dispatchEvent(new Event('visibilitychange'));
  }

  beforeEach(() => {
    stopFollowingBottom();
    awayFromBottom.value = false;
    Object.defineProperty(document, 'visibilityState', { configurable: true, value: 'visible' });
  });
  afterEach(() => { setActiveScrollElement(null); });

  it('leaves a reader who was at the live edge exactly where they were', () => {
    const el = mockScrollEl({ scrollTop: 500, scrollHeight: 1000, clientHeight: 500 });
    setActiveScrollElement(el);

    setVisibility('hidden');
    el.scrollHeight = 2000;
    setVisibility('visible');

    expect(el.scrollTop).toBe(500);
  });

  it('leaves a reader who was in history exactly where they were', () => {
    const el = mockScrollEl({ scrollTop: 200, scrollHeight: 1000, clientHeight: 500 });
    setActiveScrollElement(el);

    setVisibility('hidden');
    el.scrollHeight = 2000;
    setVisibility('visible');

    expect(el.scrollTop).toBe(200);
  });

  it('survives repeated hide/show cycles without drifting', () => {
    const el = mockScrollEl({ scrollTop: 500, scrollHeight: 1000, clientHeight: 500 });
    setActiveScrollElement(el);

    for (const height of [1500, 2000, 3000]) {
      setVisibility('hidden');
      el.scrollHeight = height;
      setVisibility('visible');
      expect(el.scrollTop).toBe(500);
    }
  });
});
