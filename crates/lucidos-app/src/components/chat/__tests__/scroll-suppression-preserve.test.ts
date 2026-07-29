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

import { mockContainer, mockScrollEl } from './scroll-test-helpers';
import { awayFromBottom, extendSuppression, getActiveScrollElement, getResizeMode, pinToBottomNow, preserveAtBottom, scrollToBottom, scrolledUp, setActiveScrollElement } from '../scrollState';

describe('scrollToBottom suppression', () => {
  beforeEach(() => {
    scrolledUp.value = false;
    vi.useFakeTimers();
  });
  afterEach(() => { vi.useRealTimers(); });

  it('scrollToBottom sets suppression to scroll mode', () => {
    const mockEl = mockScrollEl({ scrollTop: 0, scrollHeight: 2000 });
    setActiveScrollElement(mockEl);

    scrollToBottom();

    expect(getResizeMode() === 'scroll').toBe(true);
    expect(getResizeMode()).toBe('scroll');
    expect(scrolledUp.value).toBe(false);
    expect(mockEl.scrollTop).toBe(2000);

    setActiveScrollElement(null);
  });

  it('suppression clears after 500ms timeout', () => {
    const mockEl = mockScrollEl({ scrollTop: 0, scrollHeight: 2000 });
    setActiveScrollElement(mockEl);

    scrollToBottom();
    expect(getResizeMode() === 'scroll').toBe(true);

    // Advance past the 500ms window
    vi.advanceTimersByTime(500);
    expect(getResizeMode() === 'scroll').toBe(false);
    expect(getResizeMode()).toBe('ignore');

    setActiveScrollElement(null);
  });

  it('ResizeObserver must NOT escalate scrolledUp during suppression', () => {
    const mockEl = mockScrollEl({ scrollTop: 0, scrollHeight: 2000 });
    setActiveScrollElement(mockEl);

    scrolledUp.value = true;
    scrollToBottom();

    // Simulate ResizeObserver firing while in 'scroll' mode:
    // In CreateThreadView, onResize checks getResizeMode() === 'scroll'
    // and scrolls to bottom instead of setting scrolledUp
    const container = mockContainer({ scrollTop: 200, scrollHeight: 2000, clientHeight: 500 });

    if (getResizeMode() === 'scroll') {
      // New behavior: scroll to bottom during suppression
      container.scrollTop = container.scrollHeight;
    } else if (container.scrollTop + container.clientHeight < container.scrollHeight - 80) {
      scrolledUp.value = true;
    }

    // scrolledUp must stay false (scroll mode active)
    expect(scrolledUp.value).toBe(false);
    // Container was scrolled to bottom
    expect(container.scrollTop).toBe(2000);

    setActiveScrollElement(null);
  });

  it('scrollToBottom resets scrolledUp and scrolls even when user was scrolled up', () => {
    const mockEl = mockScrollEl({ scrollTop: 100, scrollHeight: 3000 });
    setActiveScrollElement(mockEl);

    scrolledUp.value = true;
    scrollToBottom();

    expect(scrolledUp.value).toBe(false);
    expect(mockEl.scrollTop).toBe(3000);

    setActiveScrollElement(null);
  });

  it('extendSuppression resets the 500ms timer', () => {
    const mockEl = mockScrollEl({ scrollTop: 0, scrollHeight: 1000 });
    setActiveScrollElement(mockEl);

    scrollToBottom();
    expect(getResizeMode() === 'scroll').toBe(true);

    // Advance 400ms, then extend
    vi.advanceTimersByTime(400);
    expect(getResizeMode() === 'scroll').toBe(true);
    extendSuppression();

    // Advance another 400ms — would have expired without extension
    vi.advanceTimersByTime(400);
    expect(getResizeMode() === 'scroll').toBe(true);

    // Advance to full 500ms from extension
    vi.advanceTimersByTime(100);
    expect(getResizeMode() === 'scroll').toBe(false);

    setActiveScrollElement(null);
  });

  it('full submit flow: scrollToBottom + new content + ResizeObserver = still at bottom', () => {
    const mockEl = mockScrollEl({ scrollTop: 200, scrollHeight: 2000 });
    setActiveScrollElement(mockEl);

    // 1. User is scrolled up
    scrolledUp.value = true;

    // 2. submit() calls scrollToBottom()
    scrollToBottom();
    expect(scrolledUp.value).toBe(false);

    // 3. New content renders → scrollHeight grows
    mockEl.scrollHeight = 2500;

    // 4. ResizeObserver fires — scroll mode scrolls to bottom
    if (getResizeMode() === 'scroll') {
      mockEl.scrollTop = mockEl.scrollHeight;
    } else if (mockEl.scrollTop + 500 < mockEl.scrollHeight - 80) {
      scrolledUp.value = true;
    }

    // scrolledUp must still be false
    expect(scrolledUp.value).toBe(false);
    // Element scrolled to new bottom
    expect(mockEl.scrollTop).toBe(2500);

    setActiveScrollElement(null);
  });
});

describe('preserveAtBottom', () => {
  beforeEach(() => {
    scrolledUp.value = false;
    awayFromBottom.value = false;
    vi.useFakeTimers();
  });
  afterEach(() => {
    // Drain the suppression timer so _resizeMode flips back to 'ignore'
    // before the next test installs fresh fake timers — otherwise the leak
    // surfaces as "expected ignore, received scroll" in the no-op test.
    vi.advanceTimersByTime(600);
    setActiveScrollElement(null);
    vi.useRealTimers();
  });

  it('engages scroll mode when user was at bottom — pin is deferred to ResizeObserver', () => {
    const el = mockScrollEl({ scrollTop: 1500, scrollHeight: 2000 });
    setActiveScrollElement(el);

    preserveAtBottom();

    expect(getResizeMode()).toBe('scroll');
    expect(scrolledUp.value).toBe(false);
    expect(awayFromBottom.value).toBe(false);
    // Asserting *absence* of a scroll write — pin is deferred to the
    // layout shift's natural ResizeObserver fire.
    expect(el.scrollTop).toBe(1500);
  });

  it('does not start a 16ms scroll loop — the keystroke-rate bug', () => {
    let writes = 0;
    let internalScrollTop = 1500;
    const el: any = {
      get scrollTop() { return internalScrollTop; },
      set scrollTop(v: number) { internalScrollTop = v; writes++; },
      scrollHeight: 2000,
      getBoundingClientRect: () => ({ width: 400, height: 600 }),
    };
    setActiveScrollElement(el);

    preserveAtBottom();
    expect(writes).toBe(0);

    // A leftover loop would write scrollTop every 16ms (~30 writes per 500ms).
    vi.advanceTimersByTime(800);
    expect(writes).toBe(0);
  });

  it('is a no-op when user has scrolled up — preserves their intent', () => {
    scrolledUp.value = true;
    const el = mockScrollEl({ scrollTop: 100, scrollHeight: 2000 });
    setActiveScrollElement(el);

    preserveAtBottom();

    expect(getResizeMode()).toBe('ignore');
    expect(el.scrollTop).toBe(100);
    expect(scrolledUp.value).toBe(true);
  });

  it('upcoming ResizeObserver fire after preserveAtBottom does not flip scrolledUp', () => {
    // Reproduces the question-card / permission-card click scenario:
    // user is at bottom → click handler runs → preserveAtBottom() →
    // re-render grows the card → ResizeObserver fires → must NOT flip scrolledUp.
    const el = mockScrollEl({ scrollTop: 1500, scrollHeight: 2000 });
    Object.assign(el, { clientHeight: 500 });
    setActiveScrollElement(el);

    preserveAtBottom();

    // Card answered-state DOM lands → child grew, scrollHeight increases.
    el.scrollHeight = 2300;

    // Mirror onResize's branch in scrollState.ts.
    if (getResizeMode() === 'scroll') {
      el.scrollTop = el.scrollHeight;
      extendSuppression();
    } else if (el.scrollTop + (el as any).clientHeight < el.scrollHeight - 80) {
      scrolledUp.value = true;
    }

    expect(scrolledUp.value).toBe(false);
    expect(el.scrollTop).toBe(2300);
  });

  it('extends suppression so a delayed re-render still pins via ResizeObserver', () => {
    const el = mockScrollEl({ scrollTop: 1500, scrollHeight: 2000 });
    setActiveScrollElement(el);

    preserveAtBottom();
    expect(getResizeMode()).toBe('scroll');

    // Re-render lands a few frames later (Preact commit + layout); mode is
    // still 'scroll' because extendSuppression set a 500ms window.
    vi.advanceTimersByTime(100);
    el.scrollHeight = 2400;
    expect(getResizeMode()).toBe('scroll');

    // The ResizeObserver fire from the child growth runs onResize, which
    // sees mode='scroll' and pins. Mirror that branch.
    if (getResizeMode() === 'scroll') {
      el.scrollTop = el.scrollHeight;
      extendSuppression();
    }

    expect(el.scrollTop).toBe(2400);
    expect(scrolledUp.value).toBe(false);
  });
});

describe('pinToBottomNow', () => {
  beforeEach(() => {
    scrolledUp.value = false;
    awayFromBottom.value = false;
    vi.useFakeTimers();
  });
  afterEach(() => {
    vi.advanceTimersByTime(600);
    setActiveScrollElement(null);
    vi.useRealTimers();
  });

  it('pins immediately and forces scrolledUp=false even when user had scrolled up', () => {
    scrolledUp.value = true;
    const el = mockScrollEl({ scrollTop: 100, scrollHeight: 3000 });
    setActiveScrollElement(el);

    pinToBottomNow();

    expect(scrolledUp.value).toBe(false);
    expect(getResizeMode()).toBe('scroll');
    expect(el.scrollTop).toBe(3000);
  });

  it('skips the scroll write when already pinned — no scroll-event cascade per fire', () => {
    let writes = 0;
    let internalScrollTop = 1500;  // at bottom: 1500 + 500 (clientHeight) = 2000 (scrollHeight)
    const el: any = {
      get scrollTop() { return internalScrollTop; },
      set scrollTop(v: number) { internalScrollTop = v; writes++; },
      scrollHeight: 2000,
      clientHeight: 500,
      getBoundingClientRect: () => ({ width: 400, height: 600 }),
    };
    setActiveScrollElement(el);

    pinToBottomNow();

    expect(writes).toBe(0);
    expect(getResizeMode()).toBe('scroll');  // mode still engaged for the upcoming RO fire
  });

  it('does not start a 16ms scroll loop — the per-keystroke vv.resize bug', () => {
    let writes = 0;
    let internalScrollTop = 100;  // far from bottom, so the guard lets the write through
    const el: any = {
      get scrollTop() { return internalScrollTop; },
      set scrollTop(v: number) { internalScrollTop = v; writes++; },
      scrollHeight: 2000,
      clientHeight: 500,
      getBoundingClientRect: () => ({ width: 400, height: 600 }),
    };
    setActiveScrollElement(el);

    pinToBottomNow();
    expect(writes).toBe(1);

    // scrollToBottom would write scrollTop every 16ms for 500ms (~30 writes).
    vi.advanceTimersByTime(800);
    expect(writes).toBe(1);
  });
});

// ---------------------------------------------------------------------------
// Active scroll element: scrollToBottom must use the registered element,
// not querySelector (which finds the wrong element on mobile or during
// component transitions).
// ---------------------------------------------------------------------------
describe('active scroll element registration', () => {
  beforeEach(() => {
    scrolledUp.value = false;
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

  it('ResizeObserver suppression works with active element', () => {
    const el = mockScrollEl({ scrollTop: 100, scrollHeight: 2000 });
    setActiveScrollElement(el);
    scrollToBottom();

    expect(getResizeMode()).toBe('scroll');
    expect(el.scrollTop).toBe(2000);

    // Content grows
    el.scrollHeight = 2500;

    // ResizeObserver fires in scroll mode → scrolls active element
    if (getResizeMode() === 'scroll') {
      el.scrollTop = el.scrollHeight;
    }
    expect(el.scrollTop).toBe(2500);
  });
});

// ---------------------------------------------------------------------------
// iOS keyboard scroll preservation
//
// On iOS Safari PWA, the virtual keyboard causes visualViewport to resize,
// which changes --app-height, resizing the scroll container. Without proper
// handling, the user loses their bottom-pinned scroll position.
//
// These tests verify the logic in MobileSwipeContainer's visualViewport
// resize handler and the onScroll suppression during scrollToBottom().
// ---------------------------------------------------------------------------
describe('iOS keyboard scroll preservation', () => {
  beforeEach(() => {
    scrolledUp.value = false;
    vi.useFakeTimers();
  });
  afterEach(() => {
    setActiveScrollElement(null);
    vi.useRealTimers();
  });

  // Simulate useAutoScroll's onScroll handler (with suppression guard)
  function onScroll(el: { scrollTop: number; clientHeight: number; scrollHeight: number }) {
    if (getResizeMode() === 'scroll') return;
    scrolledUp.value = el.scrollTop + el.clientHeight < el.scrollHeight - 80;
  }

  // Simulate useAutoScroll's onResize handler (ResizeObserver)
  // Uses scrollTop assignment like the real code — more reliable than
  // scrollTo(options) on iOS Safari during viewport transitions.
  function onResize(el: { scrollTop: number; clientHeight: number; scrollHeight: number }) {
    if (getResizeMode() === 'scroll') {
      el.scrollTop = el.scrollHeight;
      extendSuppression();
      return;
    }
    if (el.scrollTop + el.clientHeight < el.scrollHeight - 80) {
      scrolledUp.value = true;
    }
  }

  // Simulate MobileSwipeContainer's visualViewport resize handler
  function viewportResize(el: { scrollTop: number; clientHeight: number; scrollHeight: number }, newClientHeight: number) {
    const wasAtBottom = !scrolledUp.value;
    el.clientHeight = newClientHeight;
    if (wasAtBottom) {
      scrollToBottom();
    }
  }

  // Simulate useHideOnScroll focusout compensation
  function focusoutCompensation(el: { scrollTop: number; clientHeight: number; scrollHeight: number }, headerHeight: number) {
    // Spacer grows → scrollHeight increases
    el.scrollHeight += headerHeight;
    // Scroll compensation
    el.scrollTop += headerHeight;
    // Scroll event fires
    onScroll(el);
  }

  // Simulate useHideOnScroll focusin compensation
  function focusinCompensation(el: { scrollTop: number; clientHeight: number; scrollHeight: number }, headerHeight: number) {
    // Spacer collapses → scrollHeight decreases
    el.scrollHeight -= headerHeight;
    // Scroll compensation
    el.scrollTop = Math.max(0, el.scrollTop - headerHeight);
    // Scroll event fires
    onScroll(el);
  }

  it('keyboard open: maintains bottom-pinned state', () => {
    const el = mockScrollEl({ scrollTop: 0, scrollHeight: 2000 });
    Object.assign(el, { clientHeight: 600 });
    el.scrollTop = el.scrollHeight - (el as any).clientHeight; // at bottom
    setActiveScrollElement(el);

    // 1. Focus prompt input → header collapses
    focusinCompensation(el as any, 48);
    expect(scrolledUp.value).toBe(false);

    // 2. visualViewport shrinks (keyboard opens) → container shrinks
    viewportResize(el as any, 300);
    expect(scrolledUp.value).toBe(false);

    // 3. ResizeObserver fires on thread-content
    onResize(el as any);
    expect(scrolledUp.value).toBe(false);
    expect(el.scrollTop).toBe(el.scrollHeight); // scrolled to bottom
  });

  it('keyboard close: maintains bottom-pinned state', () => {
    const el = mockScrollEl({ scrollTop: 0, scrollHeight: 2000 });
    Object.assign(el, { clientHeight: 300 });
    el.scrollTop = el.scrollHeight - (el as any).clientHeight; // at bottom (with keyboard)
    setActiveScrollElement(el);

    // 1. focusout → header spacer restores
    focusoutCompensation(el as any, 48);
    expect(scrolledUp.value).toBe(false);

    // 2. visualViewport grows (keyboard closes) → container grows
    viewportResize(el as any, 600);
    expect(scrolledUp.value).toBe(false);

    // 3. ResizeObserver fires
    onResize(el as any);
    expect(scrolledUp.value).toBe(false);
    expect(el.scrollTop).toBe(el.scrollHeight);
  });

  it('keyboard open: does NOT auto-scroll when user was scrolled up', () => {
    const el = mockScrollEl({ scrollTop: 200, scrollHeight: 2000 });
    Object.assign(el, { clientHeight: 600 });
    setActiveScrollElement(el);
    scrolledUp.value = true;

    // Keyboard opens but user was scrolled up → don't force to bottom
    viewportResize(el as any, 300);
    expect(scrolledUp.value).toBe(true);
    expect(el.scrollTop).toBe(200); // position preserved
  });

  it('submit + keyboard dismiss: submitted message stays visible', () => {
    const el = mockScrollEl({ scrollTop: 200, scrollHeight: 2000 });
    Object.assign(el, { clientHeight: 300 }); // small viewport (keyboard open)
    setActiveScrollElement(el);
    scrolledUp.value = true; // user was scrolled up

    // 1. submit() calls scrollToBottom()
    scrollToBottom();
    expect(scrolledUp.value).toBe(false);
    expect(el.scrollTop).toBe(el.scrollHeight);

    // 2. Pending message renders → scrollHeight grows
    el.scrollHeight = 2200;

    // 3. ResizeObserver fires → suppression active → scrolls to new bottom
    onResize(el as any);
    expect(el.scrollTop).toBe(2200);
    expect(scrolledUp.value).toBe(false);

    // 4. focusout fires (keyboard closing) → scroll compensation
    focusoutCompensation(el as any, 48);
    // onScroll skips recalculation during suppression
    expect(scrolledUp.value).toBe(false);

    // 5. visualViewport grows (keyboard closes)
    viewportResize(el as any, 600);
    expect(scrolledUp.value).toBe(false);

    // 6. ResizeObserver fires again → still in suppression → scrolls to bottom
    onResize(el as any);
    expect(scrolledUp.value).toBe(false);
    expect(el.scrollTop).toBe(el.scrollHeight);
  });

  it('scroll events during suppression do NOT set scrolledUp', () => {
    const el = mockScrollEl({ scrollTop: 0, scrollHeight: 2000 });
    Object.assign(el, { clientHeight: 600 });
    setActiveScrollElement(el);

    // scrollToBottom starts suppression
    scrollToBottom();
    expect(getResizeMode()).toBe('scroll');

    // Programmatic scroll compensation (from useHideOnScroll focusout)
    // puts us NOT at bottom temporarily
    el.scrollTop = 500;
    onScroll(el as any);

    // scrolledUp must stay false — onScroll is suppressed
    expect(scrolledUp.value).toBe(false);
  });

  it('scroll events after suppression expires DO set scrolledUp', () => {
    const el = mockScrollEl({ scrollTop: 0, scrollHeight: 2000 });
    Object.assign(el, { clientHeight: 600 });
    setActiveScrollElement(el);

    scrollToBottom();

    // Advance past suppression window
    vi.advanceTimersByTime(500);
    expect(getResizeMode()).toBe('ignore');

    // Now a scroll event should update scrolledUp normally
    el.scrollTop = 500;
    onScroll(el as any);
    expect(scrolledUp.value).toBe(true);
  });

  it('multiple viewport resizes during keyboard animation stay at bottom', () => {
    const el = mockScrollEl({ scrollTop: 0, scrollHeight: 2000 });
    Object.assign(el, { clientHeight: 600 });
    el.scrollTop = el.scrollHeight - (el as any).clientHeight;
    setActiveScrollElement(el);

    // Simulate keyboard opening: multiple resize events with shrinking viewport
    const heights = [550, 480, 420, 350, 300];
    for (const h of heights) {
      viewportResize(el as any, h);
      onResize(el as any);
      expect(scrolledUp.value).toBe(false);
      expect(el.scrollTop).toBe(el.scrollHeight);
    }
  });

  it('submit then keyboard dismiss then response streaming', () => {
    const el = mockScrollEl({ scrollTop: 0, scrollHeight: 1500 });
    Object.assign(el, { clientHeight: 300 });
    el.scrollTop = el.scrollHeight - (el as any).clientHeight;
    setActiveScrollElement(el);

    // 1. Submit
    scrollToBottom();

    // 2. Pending message renders
    el.scrollHeight = 1700;
    onResize(el as any);
    expect(el.scrollTop).toBe(1700);

    // 3. Keyboard closes → focusout + viewport resize
    focusoutCompensation(el as any, 48);
    viewportResize(el as any, 600);
    onResize(el as any);
    expect(scrolledUp.value).toBe(false);

    // 4. Suppression expires
    vi.advanceTimersByTime(500);
    expect(getResizeMode()).toBe('ignore');

    // 5. Response starts streaming → content grows → useAutoScroll effect
    el.scrollHeight = 2000;
    // useAutoScroll effect: scrolls if not scrolledUp
    if (!scrolledUp.value) el.scrollTop = el.scrollHeight;
    expect(el.scrollTop).toBe(2000);

    // 6. More streaming
    el.scrollHeight = 2500;
    if (!scrolledUp.value) el.scrollTop = el.scrollHeight;
    expect(el.scrollTop).toBe(2500);
  });
});

// ---------------------------------------------------------------------------
// Scroll-to-bottom chevron must RE-ENGAGE tailing, not just nudge the position.
//
// Bug report: in a coding-agent thread the user clicked Approve on a permission
// card, tailing stopped, and clicking the down chevron "still didn't auto
// scroll". Root cause: the chevron handler did a bare
// `el.scrollTo({ top: scrollHeight, behavior: 'smooth' })` — a one-shot nudge
// that neither reset `scrolledUp` nor engaged the ResizeObserver suppression
// window. So with tailing already broken (`scrolledUp === true`), the chevron
// animated toward a stale bottom while content kept streaming below, and the
// deps-effect (`if (scrolledUp.value) return`) stayed parked. The fix routes
// the chevron through `scrollToBottom()`.
// ---------------------------------------------------------------------------
describe('scroll-to-bottom chevron re-engages tail', () => {
  beforeEach(() => {
    scrolledUp.value = false;
    awayFromBottom.value = false;
    vi.useFakeTimers();
  });
  afterEach(() => {
    vi.advanceTimersByTime(600);
    setActiveScrollElement(null);
    vi.useRealTimers();
  });

  it('OLD bare smooth scrollTo leaves tail broken — content keeps growing below', () => {
    // Tail already broken after Approve.
    scrolledUp.value = true;
    const el = mockScrollEl({ scrollTop: 1400, scrollHeight: 2000, clientHeight: 500 });
    setActiveScrollElement(el);

    // Old chevron handler: nudge to the current bottom, touch no signals.
    el.scrollTo({ top: el.scrollHeight, behavior: 'smooth' });
    expect(el.scrollTop).toBe(2000);
    expect(scrolledUp.value).toBe(true); // never reset → tail still broken
    expect(getResizeMode()).toBe('ignore'); // no suppression engaged

    // CC streams a fresh chunk below the (stale) bottom.
    el.scrollHeight = 2600;
    // deps-effect is parked because scrolledUp is true …
    if (!scrolledUp.value) el.scrollTop = el.scrollHeight;
    // … and onResize in ignore mode only confirms the user is "scrolled up".
    if (el.scrollTop + el.clientHeight < el.scrollHeight - 80) scrolledUp.value = true;

    expect(scrolledUp.value).toBe(true);
    expect(el.scrollTop).toBe(2000); // never followed the new content
  });

  it('NEW scrollToBottom re-engages tail — stays pinned as content streams', () => {
    // Same broken starting point.
    scrolledUp.value = true;
    const el = mockScrollEl({ scrollTop: 1400, scrollHeight: 2000, clientHeight: 500 });
    setActiveScrollElement(el);

    // New chevron handler.
    scrollToBottom();
    expect(scrolledUp.value).toBe(false); // tailing restored
    expect(getResizeMode()).toBe('scroll'); // suppression engaged
    expect(el.scrollTop).toBe(2000);

    // CC streams a fresh chunk → onResize in scroll mode pins to the new bottom.
    el.scrollHeight = 2600;
    if (getResizeMode() === 'scroll') {
      el.scrollTop = el.scrollHeight;
      extendSuppression();
    }
    expect(el.scrollTop).toBe(2600); // followed
    expect(scrolledUp.value).toBe(false);
  });
});

// ---------------------------------------------------------------------------
// Resume race: new content arriving while the user is following must snap to
// the bottom BEFORE onResize evaluates, otherwise a >80px chunk escalates
// scrolledUp=true and kills tailing on its own.
//
// This is why Approve broke tailing without any user scroll: after the approve
// gap the 500ms suppression window has expired, and CC resumes with a large
// chunk (a whole tool-call card / buffered text > 80px). The deps snap ran in a
// passive effect (after paint, after the ResizeObserver callback), so onResize
// saw the user below the new bottom and escalated first. Moving the deps snap
// into a layout effect makes it run synchronously at commit — before the
// browser delivers the ResizeObserver callback — so onResize then sees the user
// already at the bottom and leaves scrolledUp alone.
// ---------------------------------------------------------------------------
describe('resume race: snap-before-resize keeps tail alive for big chunks', () => {
  beforeEach(() => { scrolledUp.value = false; awayFromBottom.value = false; });
  afterEach(() => { setActiveScrollElement(null); });

  // The user is following (scrolledUp=false), pinned at the bottom of 2000px,
  // suppression has expired, then a 400px chunk lands in one render.
  function bigChunkArrives() {
    const el = mockScrollEl({ scrollTop: 1500, scrollHeight: 2000, clientHeight: 500 });
    setActiveScrollElement(el);
    el.scrollHeight = 2400; // big single-render growth at the bottom
    return el;
  }

  function onResizeIgnoreMode(el: { scrollTop: number; clientHeight: number; scrollHeight: number }) {
    if (el.scrollTop + el.clientHeight < el.scrollHeight - 80) scrolledUp.value = true;
  }

  it('BUG order (resize before snap) escalates scrolledUp and never follows', () => {
    const el = bigChunkArrives();
    // Passive deps-effect = runs after the ResizeObserver callback.
    onResizeIgnoreMode(el);               // 2000 < 2320 → escalate
    if (!scrolledUp.value) el.scrollTop = el.scrollHeight; // parked
    expect(scrolledUp.value).toBe(true);
    expect(el.scrollTop).toBe(1500);
  });

  it('FIX order (layout-effect snap before resize) stays pinned', () => {
    const el = bigChunkArrives();
    // Layout deps-effect = runs synchronously at commit, before onResize.
    if (!scrolledUp.value) el.scrollTop = el.scrollHeight; // snap to 2400
    onResizeIgnoreMode(el);               // 2400 >= 2320 → no escalate
    expect(scrolledUp.value).toBe(false);
    expect(el.scrollTop).toBe(2400);
  });
});
