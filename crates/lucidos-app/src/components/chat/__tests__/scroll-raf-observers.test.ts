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
    // This describe used to be "scrollToBottom continuous rAF loop" and asserted
    // the opposite at four checkpoints: content grew, and a 16ms loop dragged
    // the reader after it for the whole 500ms suppression window. Following the
    // live edge is a real behaviour again, but it is driven by the resize
    // observer honouring the flag this tap armed (see
    // scroll-follow-the-live-edge.test.ts), never by a timer. So growth with no
    // resize behind it moves nobody, however long the clock runs.
    const el = mockScrollEl({ scrollTop: 0, scrollHeight: 2000 });
    setActiveScrollElement(el);

    scrollToBottom();
    expect(el.scrollTop).toBe(2000);

    for (const height of [2100, 2300, 2500]) {
      el.scrollHeight = height;
      vi.advanceTimersByTime(100);
      expect(el.scrollTop).toBe(2000); // left exactly where the tap put them
    }
  });

  it('resolves the target at call time, so a later element swap is not chased', () => {
    const elA = mockScrollEl({ scrollTop: 0, scrollHeight: 1000 });
    const elB = mockScrollEl({ scrollTop: 0, scrollHeight: 2000 });

    setActiveScrollElement(elA);
    scrollToBottom();
    expect(elA.scrollTop).toBe(1000);

    // A layout switch mid-window used to hand the running loop the new element.
    setActiveScrollElement(elB);
    vi.advanceTimersByTime(500);
    expect(elB.scrollTop).toBe(0);

    // The chevron on the new element still works, of course.
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

// ---------------------------------------------------------------------------
// notAtTop contract: scrollToBottom() does NOT update notAtTop. The scroll
// listener handles it, so notAtTop always reflects the true DOM scroll
// position: any programmatic scrollTop assignment fires a scroll event, which
// the listener processes.
//
// These tests verify scrollToBottom()'s side of the contract: it must NOT
// touch notAtTop, leaving that entirely to the scroll listener.
// ---------------------------------------------------------------------------
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
    // Tall content: scrollToBottom() will set scrollTop=2000, but
    // it must NOT update notAtTop. The scroll event listener does that.
    const el = mockScrollEl({ scrollTop: 0, scrollHeight: 2000 });
    setActiveScrollElement(el);
    notAtTop.value = false;

    scrollToBottom();

    // scrollTop was set (scroll happened)
    expect(el.scrollTop).toBe(2000);
    // But notAtTop was NOT touched by scrollToBottom, so it stays false.
    // In a real browser the scrollTop assignment fires a scroll event, and the
    // listener sets it to true.
    expect(notAtTop.value).toBe(false);
  });

  it('does not clear notAtTop when content fits in viewport', () => {
    // If notAtTop was somehow true and content is short,
    // scrollToBottom() must not clear it: that's the listener's job.
    const el = mockScrollEl({ scrollTop: 0, scrollHeight: 50 });
    setActiveScrollElement(el);
    notAtTop.value = true;

    scrollToBottom();

    // scrollToBottom() didn't touch notAtTop, so it's still true
    // (the scroll event listener would set it to false when it fires).
    expect(notAtTop.value).toBe(true);
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
// Root cause: the transcript had two bottom-pins racing over the same expand.
// `useAutoScroll`'s layout effect ran on every render where
// eventCount/streamingBuffer/pendingCount changed, which during streaming was
// effectively every frame, and the expand click committed a render carrying
// both the new collapsed=false state and the latest chunk. The effect ran
// before the ResizeObserver in that frame, snapped scrollTop to scrollHeight,
// and by the time onResize looked the reader was already back at the bottom
// with the chevron hidden. Neither pin exists now, so the cases below assert
// the plain answer: the reader stays on what they opened.
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

  beforeEach(() => {
    stopFollowingBottom();
    awayFromBottom.value = false;
    notAtTop.value = false;
    setActiveScrollElement(null);
  });
  afterEach(() => { setActiveScrollElement(null); });

  it('the toggle shows the chevron and holds the reader still', () => {
    // Collapse then expand, from the bottom. The reader stays on the turn they
    // opened, and the chevron comes up for the content now below them.
    //
    // This needed an explicit `preserveOnToggle()` before each click, which
    // marked the reader parked so the bottom-pin would not read the expand's
    // growth as "still riding the live edge" and scroll the thing they just
    // opened off the top. Nothing pins now, so holding still is the default and
    // the toggles carry no scroll bookkeeping at all.
    const el = makeClampingEl({ scrollTop: 1000, scrollHeight: 1500, clientHeight: 500 });
    const { onScroll, onResize } = makeScrollObservers(el);
    el.addEventListener('scroll', onScroll);

    el.scrollHeight = 700;  // collapse
    onResize();
    expect(awayFromBottom.value).toBe(false);
    const afterCollapse = el.scrollTop;

    el.scrollHeight = 1500; // expand
    onResize();

    expect(awayFromBottom.value).toBe(true);
    // Held where they were: the expand is read, not scrolled past.
    expect(el.scrollTop).toBe(afterCollapse);
    expect(el.scrollTop).toBeLessThan(el.scrollHeight - el.clientHeight);
  });

  it('an expand during streaming still shows the chevron, with nothing racing it', () => {
    // The working-mode regression this describe was written for. The race was
    // between `useAutoScroll`'s layout effect (which snapped to the bottom on
    // every streamed chunk) and the ResizeObserver: whichever ran last decided
    // whether the reader saw what they had just expanded. Neither snaps now, so
    // there is no race left to lose, and streamed growth arriving alongside the
    // expand changes nothing.
    const el = makeClampingEl({ scrollTop: 1000, scrollHeight: 1500, clientHeight: 500 });
    setActiveScrollElement(el);
    const { onScroll, onResize } = makeScrollObservers(el);
    el.addEventListener('scroll', onScroll);

    el.scrollHeight = 700;  // collapse
    onResize();
    const afterCollapse = el.scrollTop;

    el.scrollHeight = 1500; // expand, plus a chunk that streamed meanwhile
    onResize();
    el.scrollHeight = 1900;
    onResize();

    expect(awayFromBottom.value).toBe(true);
    expect(el.scrollTop).toBe(afterCollapse);
  });

  it('a collapse that leaves the reader at the new bottom hides the chevron', () => {
    // After the collapse the reader IS at the bottom of the now-shrunk content,
    // because the browser clamps. The chevron must come back down: it used to
    // need the post-toggle scroll event to undo `preserveOnToggle`'s defensive
    // mark, and now onResize reconciles it directly.
    const el = makeClampingEl({ scrollTop: 1000, scrollHeight: 1500, clientHeight: 500 });
    setActiveScrollElement(el);
    const { onScroll, onResize } = makeScrollObservers(el);
    el.addEventListener('scroll', onScroll);
    awayFromBottom.value = true;

    el.scrollHeight = 700; // collapse: the browser clamp fires onScroll
    onResize();

    expect(awayFromBottom.value).toBe(false);
  });
});

// ---------------------------------------------------------------------------
// Dual-mounting visibility gate: ThreadView/CreateThreadView render twice (one
// in SplitLayout for desktop, one in MobileSwipeContainer for mobile). Both
// instances attach scroll/resize listeners that share the same notAtTop /
// awayFromBottom signals. The hidden duplicate's element has 0×0
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
    stopFollowingBottom();
    notAtTop.value = false;
    awayFromBottom.value = false;
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
// Tab visibility: leaving the tab and coming back must not move the transcript.
//
// This block was the largest single pin in the module and had nine tests behind
// it. Pinned to the bottom of a streaming response, tabbing away and back left
// the position frozen while content piled up below, so `startScrollVisibilityHandler`
// snapshotted "was the reader at the bottom" on the first hide of each cycle and
// re-pinned them on return. The handler, its first-hide-wins sentinel and its
// cold-start guard are all gone: freezing the position IS the contract now, and
// the reader comes back to exactly the thread they left.
//
// What survives is the tripwire. Nothing in the module may register a
// visibilitychange listener that moves the transcript, and a resume is not an
// explicit user action asking to go anywhere.
// ---------------------------------------------------------------------------
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
    // At the bottom: scrollTop + clientHeight = 1000 = scrollHeight.

    setVisibility('hidden');
    el.scrollHeight = 2000; // the reply streamed on while the tab was hidden
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
