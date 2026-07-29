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
import { composeHandlers } from '../promptFocus';
import { awayFromBottom, extendSuppression, getResizeMode, makeScrollObservers, notAtTop, preserveOnToggle, scrollToBottom, scrolledUp, setActiveScrollElement, startScrollVisibilityHandler, stopScrollVisibilityHandler } from '../scrollState';

describe('scrollToBottom continuous rAF loop', () => {
  beforeEach(() => {
    scrolledUp.value = false;
    vi.useFakeTimers();
  });
  afterEach(() => {
    setActiveScrollElement(null);
    vi.useRealTimers();
  });

  it('keeps scrolling on every frame during suppression window', () => {
    const el = mockScrollEl({ scrollTop: 0, scrollHeight: 2000 });
    setActiveScrollElement(el);

    scrollToBottom();
    expect(el.scrollTop).toBe(2000); // immediate scroll

    // Simulate content growing at different points during animation
    el.scrollHeight = 2100;
    vi.advanceTimersByTime(50); // ~3 frames
    expect(el.scrollTop).toBe(2100); // rAF loop caught the new height

    el.scrollHeight = 2300;
    vi.advanceTimersByTime(100); // ~6 more frames
    expect(el.scrollTop).toBe(2300);

    el.scrollHeight = 2500;
    vi.advanceTimersByTime(200); // ~12 more frames
    expect(el.scrollTop).toBe(2500);

    // After suppression expires, loop stops
    vi.advanceTimersByTime(200); // past 500ms total
    expect(getResizeMode()).toBe('ignore');

    // Scrolling no longer happens
    el.scrollHeight = 3000;
    vi.advanceTimersByTime(50);
    expect(el.scrollTop).toBe(2500); // unchanged
  });

  it('re-resolves target element on each frame', () => {
    const elA = mockScrollEl({ scrollTop: 0, scrollHeight: 1000 });
    const elB = mockScrollEl({ scrollTop: 0, scrollHeight: 2000 });

    // Start with element A
    setActiveScrollElement(elA);
    scrollToBottom();
    expect(elA.scrollTop).toBe(1000);

    // Mid-animation, active element switches to B (e.g. layout change)
    setActiveScrollElement(elB);
    vi.advanceTimersByTime(100);

    // rAF loop should now be scrolling element B
    expect(elB.scrollTop).toBe(2000);
  });

  it('new scrollToBottom call cancels previous loop', () => {
    const el = mockScrollEl({ scrollTop: 0, scrollHeight: 1000 });
    setActiveScrollElement(el);

    // First call starts a loop
    scrollToBottom();
    expect(el.scrollTop).toBe(1000);

    // Content grows, second call replaces the loop
    el.scrollHeight = 2000;
    scrollToBottom();
    expect(el.scrollTop).toBe(2000);

    // Only one loop should be running — advance and check
    el.scrollHeight = 3000;
    vi.advanceTimersByTime(100);
    expect(el.scrollTop).toBe(3000);
  });

  it('scrolledUp stays false throughout the entire loop', () => {
    const el = mockScrollEl({ scrollTop: 0, scrollHeight: 2000 });
    setActiveScrollElement(el);

    scrollToBottom();

    // Check at multiple points during the 500ms window
    const checkpoints = [50, 100, 150, 200, 300, 400];
    let elapsed = 0;
    for (const target of checkpoints) {
      vi.advanceTimersByTime(target - elapsed);
      elapsed = target;
      expect(scrolledUp.value).toBe(false);
    }
  });

  it('reconciles awayFromBottom on loop exit when content grew past the last scroll', () => {
    // Browser-accurate clamping is load-bearing: without it, `scrollTop =
    // scrollHeight` leaves scrollTop higher than max and the post-grow
    // off-bottom state is invisible to the reconciler.
    const el = {
      _scrollTop: 1500,
      get scrollTop() { return this._scrollTop; },
      set scrollTop(v: number) {
        const max = Math.max(0, this.scrollHeight - this.clientHeight);
        this._scrollTop = Math.min(v, max);
      },
      scrollHeight: 2000,
      clientHeight: 500,
      getBoundingClientRect: () => ({ width: 400, height: 600 }),
    } as any;
    setActiveScrollElement(el);

    scrollToBottom();
    expect(el.scrollTop).toBe(1500);
    expect(awayFromBottom.value).toBe(false);

    vi.advanceTimersByTime(480);
    expect(el.scrollTop).toBe(1500);
    expect(awayFromBottom.value).toBe(false);
    expect(getResizeMode()).toBe('scroll');

    // Grow content between the loop's last in-window iteration and exit.
    // Real-world cause: a child whose container box stayed the same so RO
    // didn't fire — the rAF loop was the only thing keeping us pinned.
    vi.advanceTimersByTime(16);
    el.scrollHeight = 2400;
    vi.advanceTimersByTime(20);

    expect(getResizeMode()).toBe('ignore');
    expect(el.scrollTop).toBe(1500);
    expect(awayFromBottom.value).toBe(true);
  });
});

// ---------------------------------------------------------------------------
// notAtTop contract: scrollToBottom() does NOT update notAtTop. The scroll
// listener handles it — updating notAtTop BEFORE the suppression guard so it
// always reflects the true DOM scroll position. This makes impossible states
// impossible: any programmatic scrollTop assignment fires a scroll event,
// which the listener processes regardless of suppression mode.
//
// These tests verify scrollToBottom()'s side of the contract: it must NOT
// touch notAtTop, leaving that entirely to the scroll listener.
// ---------------------------------------------------------------------------
describe('scrollToBottom does not touch notAtTop (scroll listener owns it)', () => {
  beforeEach(() => {
    scrolledUp.value = false;
    notAtTop.value = false;
    vi.useFakeTimers();
  });
  afterEach(() => {
    setActiveScrollElement(null);
    vi.useRealTimers();
  });

  it('does not set notAtTop even when scrolled far from top', () => {
    // Tall content — scrollToBottom() will set scrollTop=2000, but
    // it must NOT update notAtTop. The scroll event listener does that.
    const el = mockScrollEl({ scrollTop: 0, scrollHeight: 2000 });
    setActiveScrollElement(el);
    notAtTop.value = false;

    scrollToBottom();

    // scrollTop was set (scroll happened)
    expect(el.scrollTop).toBe(2000);
    // But notAtTop was NOT touched by scrollToBottom — it stays false
    // In real browser: the scrollTop assignment fires a scroll event,
    // and the listener (not suppressed for notAtTop) sets it to true.
    expect(notAtTop.value).toBe(false);
  });

  it('does not clear notAtTop when content fits in viewport', () => {
    // If notAtTop was somehow true and content is short,
    // scrollToBottom() must not clear it — that's the listener's job.
    const el = mockScrollEl({ scrollTop: 0, scrollHeight: 50 });
    setActiveScrollElement(el);
    notAtTop.value = true;

    scrollToBottom();

    // scrollToBottom() didn't touch notAtTop — it's still true
    // (scroll event listener would set it to false when it fires)
    expect(notAtTop.value).toBe(true);
  });

  it('continuous scroll loop also does not touch notAtTop', () => {
    const el = mockScrollEl({ scrollTop: 0, scrollHeight: 50 });
    setActiveScrollElement(el);
    notAtTop.value = false;

    scrollToBottom();

    // Content grows
    el.scrollHeight = 2000;
    vi.advanceTimersByTime(50);

    // Loop scrolled to bottom
    expect(el.scrollTop).toBe(2000);
    // But notAtTop still untouched by scrollToBottom/loop
    expect(notAtTop.value).toBe(false);
  });
});

// ---------------------------------------------------------------------------
// composeHandlers contract — used by mode toggles + attach-photo button to
// keep the iOS keyboard open across the re-render those actions trigger.
// Action buttons (Send / Apply / etc.) deliberately do NOT use this
// helper — see installActionBtnBlurListener() in promptFocus.ts.
// ---------------------------------------------------------------------------
describe('composeHandlers focuses before action', () => {
  it('composeHandlers focuses prompt before action', () => {
    // Verify the composeHandlers contract: focusFn runs before action
    const order: string[] = [];
    const focusFn = () => order.push('focus');
    const action = () => order.push('action');

    const handlers = composeHandlers(action, focusFn);

    // Simulate touchend (iOS path)
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

    // Simulate click (desktop path, no prior touch)
    handlers.onClick();
    expect(order).toEqual(['focus', 'action']);
  });

  it('touchend prevents subsequent click from double-firing', () => {
    let actionCount = 0;
    const focusFn = () => {};
    const action = () => actionCount++;

    const handlers = composeHandlers(action, focusFn);

    // Touch then click (iOS fires both)
    handlers.onTouchEnd({ preventDefault: vi.fn() } as any);
    handlers.onClick();

    expect(actionCount).toBe(1); // action only ran once
  });
});

// ---------------------------------------------------------------------------
// Last-response-panel collapse → expand: chevron must reappear on expand.
//
// User-reported regression: pinned to the bottom of a thread whose last
// response panel is in Working mode (Lucidos / CC streaming), the user
// clicks the response-panel header to collapse, then again to expand —
// the chevron should appear because the just-rendered body extends below
// the viewport, but it stays hidden.
//
// Root cause: useAutoScroll's effect runs on every render where
// eventCount/streamingBuffer/pendingCount changes — during streaming that's
// effectively every frame. The expand click commits a render that includes
// (a) the new collapsed=false state and (b) the latest streaming chunk's
// state changes. useEffect runs before ResizeObserver in the same frame, so
// the auto-scroll fires first, sets el.scrollTop = el.scrollHeight, and by
// the time onResize runs the user is already pinned to the (new) bottom.
// onResize sees isAtBottom=true, the chevron stays hidden.
// ---------------------------------------------------------------------------
describe('response-panel collapse → expand re-shows the scroll-to-bottom chevron', () => {
  /** Browser-accurate scroll element: assigning scrollTop above max clamps it
   *  and synchronously fires a scroll event. Lets us simulate the collapse
   *  step (scrollHeight shrinks → browser clamps → scroll event fires)
   *  without rolling a separate event-dispatch helper. */
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
        // Browsers clamp scrollTop synchronously when scrollHeight shrinks
        // below scrollTop + clientHeight. Mirror that here so onResize/onScroll
        // see the same world a real browser would.
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

  /** useAutoScroll's effect: runs every time eventCount/streamingBuffer/
   *  pendingCount changes, scrolls to bottom unless scrolledUp is set. */
  function autoScrollEffect(el: any) {
    if (scrolledUp.value) return;
    el.scrollTop = el.scrollHeight;
  }

  beforeEach(() => {
    // Drain any leftover suppression so _resizeMode starts at 'ignore'.
    // Stale state from prior tests: a previous beforeEach may have switched
    // away from fake timers without firing the suppression-clearing setTimeout,
    // leaving _resizeMode='scroll' globally. Without this drain, onResize takes
    // the scroll-mode branch (scroll-to-bottom + return) instead of the chevron
    // escalation branch and the test flakes by execution order.
    vi.useFakeTimers();
    extendSuppression();           // schedules a fresh setTimeout under fake time
    vi.advanceTimersByTime(600);   // fires it → _resizeMode = 'ignore'
    vi.useRealTimers();
    awayFromBottom.value = false;
    scrolledUp.value = false;
    notAtTop.value = false;
    setActiveScrollElement(null);
  });
  afterEach(() => { setActiveScrollElement(null); });

  it('non-streaming: ResizeObserver alone escalates the chevron after expand', () => {
    // Baseline: with no auto-scroll racing, the existing onResize logic does
    // the right thing. Captures the contract that the streaming-mode test
    // breaks below.
    const el = makeClampingEl({ scrollTop: 1000, scrollHeight: 1500, clientHeight: 500 });
    const { onScroll, onResize } = makeScrollObservers(el);
    el.addEventListener('scroll', onScroll);

    el.scrollHeight = 700;  // collapse
    onResize();
    expect(awayFromBottom.value).toBe(false);

    el.scrollHeight = 1500; // expand
    onResize();

    expect(awayFromBottom.value).toBe(true);
    expect(scrolledUp.value).toBe(true);
  });

  it('streaming-mode: chevron must appear after expand even when auto-scroll races onResize', () => {
    // Reproduces the working-mode regression. Sequence inside a single
    // render frame on expand: useEffect (auto-scroll) → ResizeObserver.
    // If we don't suppress the auto-scroll, the user is snapped back to
    // the bottom before onResize can escalate the chevron.
    const el = makeClampingEl({ scrollTop: 1000, scrollHeight: 1500, clientHeight: 500 });
    setActiveScrollElement(el);
    const { onScroll, onResize } = makeScrollObservers(el);
    el.addEventListener('scroll', onScroll);

    // Streaming has been keeping the user pinned — auto-scroll fires every
    // frame because new TextStreamed events keep arriving.
    autoScrollEffect(el);
    expect(el.scrollTop).toBe(1000); // already at bottom

    // Click 1: collapse the last response panel. ChatExchange re-renders;
    // the body div is removed.
    preserveOnToggle();
    el.scrollHeight = 700;  // post-render layout
    autoScrollEffect(el);   // useEffect from latest streaming chunk
    onResize();             // ResizeObserver after layout

    // Click 2: expand. ChatExchange re-renders; the body re-appears with
    // all the text accumulated during the collapse window.
    preserveOnToggle();
    el.scrollHeight = 1500; // post-render layout
    autoScrollEffect(el);   // useEffect from latest streaming chunk —
                            // MUST be suppressed by preserveOnToggle
    onResize();

    // The chevron must be visible now: the body content extends well below
    // the user's anchor. Without preserveOnToggle, autoScrollEffect would
    // have snapped scrollTop to 1000 and onResize would see isAtBottom=true.
    expect(awayFromBottom.value).toBe(true);
    expect(scrolledUp.value).toBe(true);
    // And the user is still anchored where they were — auto-scroll did NOT
    // sneakily pin them to the new bottom.
    expect(el.scrollTop).toBeLessThan(el.scrollHeight - el.clientHeight);
  });

  it('preserveOnToggle: collapse from at-bottom does not strand scrolledUp=true', () => {
    // After collapse, the user IS at the bottom of the now-shrunk content
    // (browser clamping). preserveOnToggle's defensive scrolledUp=true must
    // be reconciled back to false by the post-toggle scroll event so the
    // next streaming chunk auto-scrolls normally.
    const el = makeClampingEl({ scrollTop: 1000, scrollHeight: 1500, clientHeight: 500 });
    setActiveScrollElement(el);
    const { onScroll, onResize } = makeScrollObservers(el);
    el.addEventListener('scroll', onScroll);

    preserveOnToggle();
    el.scrollHeight = 700; // collapse — browser clamp fires onScroll
    onResize();

    // Browser-clamp scroll event ran onScroll, which is the both-ways path:
    // user is at the new bottom → scrolledUp cleared, awayFromBottom cleared.
    expect(scrolledUp.value).toBe(false);
    expect(awayFromBottom.value).toBe(false);
  });
});

// ---------------------------------------------------------------------------
// Dual-mounting visibility gate: ThreadView/CreateThreadView render twice (one
// in SplitLayout for desktop, one in MobileSwipeContainer for mobile). Both
// instances attach scroll/resize listeners that share the same notAtTop /
// awayFromBottom / scrolledUp signals. The hidden duplicate's element has 0×0
// dimensions — its handlers see "not scrollable" and "at bottom" and would
// clobber the visible instance's correct values. makeScrollObservers gates all
// signal writes on isElementVisible(el) so only the visible copy can mutate.
//
// Bug repro: focus a long thread → expected scroll-to-top chevron is missing
// because the hidden duplicate's syncNotAtTop fired last with notAtTop=false.
// ---------------------------------------------------------------------------
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
      // isElementVisible(): zero dimensions on the element itself short-circuit
      // to false. Real hidden duplicates inherit display:none from an ancestor
      // (.split-layout on mobile, .mobile-swipe-wrapper on desktop), which
      // collapses the element's own getBoundingClientRect to 0×0.
      getBoundingClientRect: () => opts.visible
        ? { width: 400, height: 600 }
        : { width: 0, height: 0 },
      parentElement: null,
    };
    return el;
  }

  beforeEach(() => {
    notAtTop.value = false;
    awayFromBottom.value = false;
    scrolledUp.value = false;
  });

  it('hidden el onResize does not clear notAtTop set by visible el', () => {
    // Visible: long thread, scrolled to bottom — chevron should be visible.
    const visible = makeEl({ visible: true, scrollTop: 4500, scrollHeight: 5200, clientHeight: 700 });
    const hidden = makeEl({ visible: false });

    const visibleObs = makeScrollObservers(visible);
    const hiddenObs = makeScrollObservers(hidden);

    visibleObs.onResize();
    expect(notAtTop.value).toBe(true);

    // Hidden duplicate's ResizeObserver fires (e.g., children render). Without
    // the visibility gate it ran syncNotAtTop with isScrollable=false →
    // notAtTop=false → chevron disappeared.
    hiddenObs.onResize();
    expect(notAtTop.value).toBe(true);
  });

  it('hidden el onScroll does not clear awayFromBottom set by visible el', () => {
    const visible = makeEl({ visible: true, scrollTop: 100, scrollHeight: 5000, clientHeight: 700 });
    const hidden = makeEl({ visible: false });

    const visibleObs = makeScrollObservers(visible);
    const hiddenObs = makeScrollObservers(hidden);

    // User scrolled up in the visible thread → awayFromBottom should latch true.
    visibleObs.onScroll();
    expect(awayFromBottom.value).toBe(true);

    // Hidden duplicate fires a stray scroll event (e.g., browser auto-clamps
    // scrollTop on layout flip). Without the gate, awayFromBottom flips back
    // to false because the hidden el reads as visually-at-bottom.
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

// ---------------------------------------------------------------------------
// Tab visibility: auto-scroll must survive tabbing away during streaming.
//
// User-reported regression: pinned to the bottom of a streaming response,
// switch to another browser tab, come back — scroll position is frozen
// where it was and new content piles up below the viewport without the
// auto-scroll catching up.
//
// Root cause: while the tab is hidden, browsers throttle layout / rendering
// and `el.scrollTop = el.scrollHeight` inside useAutoScroll's deps-effect
// doesn't realize as an actual scroll. On return, the ResizeObserver fires
// for the accumulated child growth and onResize sees scrollTop is far below
// scrollHeight, escalating scrolledUp=true. Future deps-effect fires then
// skip auto-scroll, locking the user out of bottom-pinned mode.
//
// Fix: capture wasAtBottom (snapshot of !scrolledUp.value) when the tab
// goes hidden, and re-pin via scrollToBottom() on return if so. The
// scrollToBottom() 500ms suppression window prevents any racing RO fire
// from escalating scrolledUp back to true.
// ---------------------------------------------------------------------------
describe('tab visibility — auto-scroll persists across hidden/visible cycles', () => {
  function setVisibility(state: 'hidden' | 'visible') {
    Object.defineProperty(document, 'visibilityState', { configurable: true, value: state });
    document.dispatchEvent(new Event('visibilitychange'));
  }

  beforeEach(() => {
    scrolledUp.value = false;
    awayFromBottom.value = false;
    vi.useFakeTimers();
    Object.defineProperty(document, 'visibilityState', { configurable: true, value: 'visible' });
  });
  afterEach(() => {
    stopScrollVisibilityHandler();
    vi.advanceTimersByTime(600); // drain any pending suppression timer
    setActiveScrollElement(null);
    vi.useRealTimers();
  });

  it('returns to bottom after hidden→visible if user was at bottom when hiding', () => {
    startScrollVisibilityHandler();
    const el = mockScrollEl({ scrollTop: 500, scrollHeight: 1000, clientHeight: 500 });
    setActiveScrollElement(el);
    // User is at the bottom: scrollTop+clientHeight = 1000 = scrollHeight.

    setVisibility('hidden');
    // Content streams in while tab is hidden — scrollHeight grows but the
    // hidden-tab layout throttling means scrollTop stays at its old value.
    el.scrollHeight = 2000;

    setVisibility('visible');

    // Handler captured wasAtBottom=true on hide → re-pins on return.
    expect(el.scrollTop).toBe(2000);
    expect(scrolledUp.value).toBe(false);
    // Suppression engaged so a subsequent RO fire can't escalate.
    expect(getResizeMode()).toBe('scroll');
  });

  it('does NOT re-pin if user had scrolled up before hiding', () => {
    startScrollVisibilityHandler();
    const el = mockScrollEl({ scrollTop: 200, scrollHeight: 1000, clientHeight: 500 });
    setActiveScrollElement(el);
    scrolledUp.value = true; // user was reading history

    setVisibility('hidden');
    el.scrollHeight = 2000;

    setVisibility('visible');

    // Captured wasAtBottom=false → preserves user's reading position.
    expect(el.scrollTop).toBe(200);
    expect(scrolledUp.value).toBe(true);
  });

  it('re-pin survives a racing onResize that would otherwise escalate scrolledUp', () => {
    // Reproduces the exact lock-out: RO fires on resume (before or after
    // visibilitychange fires) and would normally escalate scrolledUp=true,
    // killing future auto-scrolls. The 500ms suppression set by
    // scrollToBottom() must guard the re-pin against this race.
    startScrollVisibilityHandler();
    const el = mockScrollEl({ scrollTop: 500, scrollHeight: 1000, clientHeight: 500 });
    setActiveScrollElement(el);

    setVisibility('hidden');
    el.scrollHeight = 3000;

    setVisibility('visible');
    // Suppression engaged → RO sees mode='scroll' and scrolls instead of
    // setting scrolledUp.
    expect(getResizeMode()).toBe('scroll');

    // Simulate the racing RO fire that mirrors makeScrollObservers.onResize.
    if (getResizeMode() === 'scroll') {
      el.scrollTop = el.scrollHeight;
      extendSuppression();
    } else if (el.scrollTop + el.clientHeight < el.scrollHeight - 80) {
      scrolledUp.value = true;
    }

    expect(scrolledUp.value).toBe(false);
    expect(el.scrollTop).toBe(3000);
  });

  it('handles repeated hide/show cycles correctly', () => {
    startScrollVisibilityHandler();
    const el = mockScrollEl({ scrollTop: 500, scrollHeight: 1000, clientHeight: 500 });
    setActiveScrollElement(el);

    // Cycle 1: at bottom → hide → grow → show → re-pin
    setVisibility('hidden');
    el.scrollHeight = 1500;
    setVisibility('visible');
    expect(el.scrollTop).toBe(1500);

    // Drain suppression before next cycle
    vi.advanceTimersByTime(600);

    // Cycle 2: scroll up → hide → show → no re-pin
    scrolledUp.value = true;
    el.scrollTop = 100;
    setVisibility('hidden');
    el.scrollHeight = 2000;
    setVisibility('visible');
    expect(el.scrollTop).toBe(100);
    expect(scrolledUp.value).toBe(true);

    vi.advanceTimersByTime(600);

    // Cycle 3: scroll back to bottom → hide → grow → show → re-pin again
    scrolledUp.value = false;
    el.scrollTop = el.scrollHeight - el.clientHeight; // user scrolled to bottom
    setVisibility('hidden');
    el.scrollHeight = 3000;
    setVisibility('visible');
    expect(el.scrollTop).toBe(3000);
    expect(scrolledUp.value).toBe(false);
  });

  it('stop handler tears down the listener', () => {
    startScrollVisibilityHandler();
    const el = mockScrollEl({ scrollTop: 500, scrollHeight: 1000, clientHeight: 500 });
    setActiveScrollElement(el);

    setVisibility('hidden');
    stopScrollVisibilityHandler();
    el.scrollHeight = 2000;
    setVisibility('visible');

    // Listener was removed → no re-pin happens
    expect(el.scrollTop).toBe(500);
  });

  it('does not break when no active scroll element is set', () => {
    startScrollVisibilityHandler();
    // No setActiveScrollElement call

    setVisibility('hidden');
    setVisibility('visible');

    // Should not throw, no state changes
    expect(scrolledUp.value).toBe(false);
  });

  it('does NOT engage scroll mode when no active element exists on hide (cold start)', () => {
    // User is on Settings or a non-thread view — no .thread-content registered.
    // scrolledUp defaults to false, so a naive `!scrolledUp.value` capture
    // would set wasAtBottom=true and on resume call pinToBottomNow(), which
    // sets _resizeMode='scroll' globally for 500ms. If the user opens a thread
    // inside that window, the thread's RO sees mode='scroll' and snaps to
    // bottom, overriding any saved scroll position useScrollMemory would
    // otherwise restore. The active-element guard prevents the capture.
    startScrollVisibilityHandler();
    // No setActiveScrollElement — there's no chat view mounted.
    scrolledUp.value = false; // default

    setVisibility('hidden');
    setVisibility('visible');

    // No spurious mode leak — _resizeMode stays 'ignore'.
    expect(getResizeMode()).toBe('ignore');
  });

  it('first-hide-wins: double hidden fire does not lose the original capture', () => {
    // iOS can fire visibilitychange to hidden multiple times during background
    // transitions. If a stray ResizeObserver fires between the two hidden
    // events and escalates scrolledUp=true, the second capture would store
    // wasAtBottom=false and the eventual visible→re-pin would no-op,
    // re-surfacing the original bug. First-hide-wins via the null sentinel
    // protects against this.
    startScrollVisibilityHandler();
    const el = mockScrollEl({ scrollTop: 500, scrollHeight: 1000, clientHeight: 500 });
    setActiveScrollElement(el);
    scrolledUp.value = false; // at bottom

    setVisibility('hidden'); // first hide → captures wasAtBottom=true
    // Simulate the stray RO escalation that triggered the original bug:
    scrolledUp.value = true;
    setVisibility('hidden'); // second hide → MUST NOT overwrite the capture
    // Tab becomes visible — the original capture should still drive re-pin.
    el.scrollHeight = 2500;
    setVisibility('visible');

    // pinToBottomNow ignored the post-capture scrolledUp=true because the
    // first-hide capture saved the user's true intent.
    expect(el.scrollTop).toBe(2500);
    expect(scrolledUp.value).toBe(false);
  });
});
