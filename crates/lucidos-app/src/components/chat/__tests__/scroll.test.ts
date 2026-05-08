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

import { scrolledUp, awayFromBottom, notAtTop, scrollToBottom, getResizeMode, extendSuppression, setActiveScrollElement, getActiveScrollElement, makeScrollObservers } from '../scrollState';
import { withScrollAnchor } from '../CreateThreadView';
import { composeHandlers } from '../promptFocus';

// ---------------------------------------------------------------------------
// Mock DOM helpers
// ---------------------------------------------------------------------------

class MockMutationObserver {
  observe = vi.fn();
  disconnect = vi.fn();
  constructor(_cb?: any) {}
}

function useMockMO() {
  const orig = globalThis.MutationObserver;
  (globalThis as any).MutationObserver = MockMutationObserver;
  return () => { (globalThis as any).MutationObserver = orig; };
}

/** Minimal mock for setActiveScrollElement — must have getBoundingClientRect
 *  so isElementVisible() works inside scrollToBottom(). scrollToBottom() uses
 *  direct scrollTop assignment which is more reliable than scrollTo(options) on
 *  iOS Safari during viewport transitions. scrollTo() kept for button tests. */
function mockScrollEl(opts: { scrollTop?: number; scrollHeight?: number }) {
  const el = {
    scrollTop: opts.scrollTop ?? 0,
    scrollHeight: opts.scrollHeight ?? 1000,
    getBoundingClientRect: () => ({ width: 400, height: 600 }),
    scrollTo(arg: any) {
      if (typeof arg === 'object' && arg.top !== undefined) el.scrollTop = arg.top;
    },
  };
  return el as any;
}

function mockContainer(opts: {
  scrollTop?: number;
  scrollHeight?: number;
  clientHeight?: number;
} = {}) {
  const el = {
    scrollTop: opts.scrollTop ?? 0,
    scrollHeight: opts.scrollHeight ?? 1000,
    clientHeight: opts.clientHeight ?? 500,
    style: { overflow: '' } as Record<string, string>,
    addEventListener: vi.fn(),
    removeEventListener: vi.fn(),
    scrollTo: vi.fn((arg: any) => {
      if (typeof arg === 'object') el.scrollTop = arg.top;
    }),
    closest: vi.fn(() => null),
    contains: vi.fn(() => false),
  };
  return el;
}

function mockAnchorInContainer(container: ReturnType<typeof mockContainer>, offsetTop = 200) {
  return {
    offsetTop,
    closest: vi.fn(() => container),
    isConnected: true,
  };
}

function mockDynamicAnchor(container: ReturnType<typeof mockContainer>, initialOffset: number) {
  let offset = initialOffset;
  return {
    get offsetTop() { return offset; },
    set _offset(v: number) { offset = v; },
    closest: vi.fn(() => container),
    isConnected: true,
    _setOffset(v: number) { offset = v; },
  };
}

// ---------------------------------------------------------------------------
// scrolledUp detection
// ---------------------------------------------------------------------------
describe('scrolledUp detection', () => {
  beforeEach(() => { scrolledUp.value = false; });

  // Same math as useAutoScroll's scroll handler
  function isScrolledUp(scrollTop: number, clientHeight: number, scrollHeight: number) {
    return scrollTop + clientHeight < scrollHeight - 80;
  }

  it('is false initially', () => {
    expect(scrolledUp.value).toBe(false);
  });

  it('detects user is at the bottom (within 80px threshold)', () => {
    expect(isScrolledUp(500, 500, 1000)).toBe(false);
    expect(isScrolledUp(450, 500, 1000)).toBe(false);
    expect(isScrolledUp(420, 500, 1000)).toBe(false);
  });

  it('detects user has scrolled up (beyond 80px threshold)', () => {
    expect(isScrolledUp(419, 500, 1000)).toBe(true);
    expect(isScrolledUp(0, 500, 1000)).toBe(true);
    expect(isScrolledUp(200, 500, 1000)).toBe(true);
  });

  it('handles small content that fits in viewport', () => {
    expect(isScrolledUp(0, 500, 400)).toBe(false);
    expect(isScrolledUp(0, 500, 500)).toBe(false);
  });
});

// ---------------------------------------------------------------------------
// awayFromBottom — drives chevron visibility independent of stickiness
// ---------------------------------------------------------------------------
// Chevron must appear immediately when the user scrolls up (any pixels), even
// inside the 80px stickiness window where scrolledUp stays false so that
// auto-scroll can still snap back during streaming.
describe('awayFromBottom detection', () => {
  beforeEach(() => { awayFromBottom.value = false; scrolledUp.value = false; });

  function isVisuallyAtBottom(scrollTop: number, clientHeight: number, scrollHeight: number) {
    return scrollTop + clientHeight >= scrollHeight - 2;
  }

  it('is false when exactly at the bottom', () => {
    expect(isVisuallyAtBottom(500, 500, 1000)).toBe(true);
  });

  it('is true after a small scroll-up that is still inside the stickiness window', () => {
    // 20px from bottom: well inside the 80px stickiness threshold so scrolledUp
    // would be false, but the chevron must still appear immediately.
    expect(isVisuallyAtBottom(480, 500, 1000)).toBe(false);
  });

  it('is false when content fits in viewport (no scroll possible)', () => {
    expect(isVisuallyAtBottom(0, 500, 400)).toBe(true);
    expect(isVisuallyAtBottom(0, 500, 500)).toBe(true);
  });

  // Mirrors the resize handler: clear-only on resize so streaming content
  // growth doesn't trip a one-frame flicker before useEffect snaps to bottom.
  it('resize clears awayFromBottom when shrink leaves user visually at bottom', () => {
    awayFromBottom.value = true;
    // Content shrinks from 1000 → 600; user's scrollTop=100, clientHeight=500.
    // 100+500 = 600 = scrollHeight → visually at bottom now.
    if (awayFromBottom.value && isVisuallyAtBottom(100, 500, 600)) {
      awayFromBottom.value = false;
    }
    expect(awayFromBottom.value).toBe(false);
  });

  // Mirrors the resize handler: escalation is gated on the 80px stickiness
  // window, not on isVisuallyAtBottom (2px). Streaming tokens grow content
  // by ~20px each, well within the window — so this branch never trips
  // mid-stream and there is no one-frame chevron flicker. Larger growths
  // (panel expand, multi-line code block) cross the window and escalate.
  function onResizeEscalation(scrollTop: number, clientHeight: number, scrollHeight: number) {
    const isAtBottom = scrollTop + clientHeight >= scrollHeight - 80;
    if (!isAtBottom) {
      scrolledUp.value = true;
      awayFromBottom.value = true;
    }
    if (awayFromBottom.value && isVisuallyAtBottom(scrollTop, clientHeight, scrollHeight)) {
      awayFromBottom.value = false;
    }
  }

  it('resize escalates awayFromBottom when growth exceeds the 80px stickiness window (panel expand)', () => {
    // User was at the visual bottom of 1000px content. A panel expansion
    // adds 500px below the user's anchor — scrollTop stays at 500, but
    // scrollHeight is now 1500. User is 500px from the bottom, well past
    // the 80px window: chevron must appear without waiting for a scroll.
    onResizeEscalation(500, 500, 1500);
    expect(scrolledUp.value).toBe(true);
    expect(awayFromBottom.value).toBe(true);
  });

  it('resize does NOT escalate awayFromBottom for small streaming-growth (within 80px window)', () => {
    // Content grew from 1000 → 1050 during streaming. User stays within
    // the 80px stickiness window (50 < 80) — useEffect will snap back to
    // bottom on the same frame, so escalating here would just flicker.
    onResizeEscalation(500, 500, 1050);
    expect(scrolledUp.value).toBe(false);
    expect(awayFromBottom.value).toBe(false);
  });

  it('resize clears awayFromBottom when shrink leaves user visually at bottom', () => {
    awayFromBottom.value = true;
    // Content shrinks from 1100 → 600; user's scrollTop=100, clientHeight=500.
    // 100+500 = 600 = scrollHeight → visually at bottom now.
    onResizeEscalation(100, 500, 600);
    expect(awayFromBottom.value).toBe(false);
  });
});

// ---------------------------------------------------------------------------
// scroll-to-bottom button visibility
// ---------------------------------------------------------------------------
describe('scroll-to-bottom button visibility', () => {
  beforeEach(() => { awayFromBottom.value = false; });

  it('button class is not visible when awayFromBottom is false', () => {
    const className = `scroll-to-bottom${awayFromBottom.value ? ' visible' : ''}`;
    expect(className).toBe('scroll-to-bottom');
  });

  it('button class is visible when awayFromBottom is true', () => {
    awayFromBottom.value = true;
    const className = `scroll-to-bottom${awayFromBottom.value ? ' visible' : ''}`;
    expect(className).toBe('scroll-to-bottom visible');
  });

  it('clicking button scrolls to bottom', () => {
    const el = mockContainer({ scrollHeight: 2000 });
    el.scrollTo({ top: el.scrollHeight, behavior: 'smooth' });
    expect(el.scrollTo).toHaveBeenCalledWith({ top: 2000, behavior: 'smooth' });
  });
});

// ---------------------------------------------------------------------------
// withScrollAnchor — scroll position preservation
// ---------------------------------------------------------------------------
describe('withScrollAnchor', () => {
  it('calls fn even when anchor is null', () => {
    const fn = vi.fn();
    withScrollAnchor(null, fn);
    expect(fn).toHaveBeenCalledOnce();
  });

  it('calls fn even when anchor has no container', () => {
    const fn = vi.fn();
    withScrollAnchor({ closest: () => null } as any, fn);
    expect(fn).toHaveBeenCalledOnce();
  });

  it('preserves scroll position when anchor does not move', () => {
    const restore = useMockMO();
    const container = mockContainer({ scrollTop: 300 });
    const anchor = mockAnchorInContainer(container, 200);

    withScrollAnchor(anchor as any, () => {});

    expect(container.scrollTop).toBe(300);
    restore();
  });

  it('adjusts scroll position when anchor moves down', () => {
    const restore = useMockMO();
    const container = mockContainer({ scrollTop: 300 });
    const a = mockDynamicAnchor(container, 200);

    withScrollAnchor(a as any, () => { a._setOffset(350); });

    // 300 + (350 - 200) = 450
    expect(container.scrollTop).toBe(450);
    restore();
  });

  it('adjusts scroll position when anchor moves up', () => {
    const restore = useMockMO();
    const container = mockContainer({ scrollTop: 500 });
    const a = mockDynamicAnchor(container, 400);

    withScrollAnchor(a as any, () => { a._setOffset(300); });

    // 500 + (300 - 400) = 400
    expect(container.scrollTop).toBe(400);
    restore();
  });

  it('freezes container overflow during mutation', () => {
    const restore = useMockMO();
    const container = mockContainer({ scrollTop: 100 });
    container.style.overflow = '';
    const anchor = mockAnchorInContainer(container, 50);
    let overflowDuringFn = '';

    withScrollAnchor(anchor as any, () => {
      overflowDuringFn = container.style.overflow;
    });

    expect(overflowDuringFn).toBe('hidden');
    restore();
  });

  it('restores overflow after mutation', async () => {
    const restore = useMockMO();
    const origRAF = globalThis.requestAnimationFrame;
    (globalThis as any).requestAnimationFrame = (cb: any) => { cb(); return 0; };
    const container = mockContainer({ scrollTop: 100 });
    container.style.overflow = 'auto';
    const anchor = mockAnchorInContainer(container, 50);

    withScrollAnchor(anchor as any, () => {});

    await new Promise(r => setTimeout(r, 0));
    expect(container.style.overflow).toBe('auto');

    (globalThis as any).requestAnimationFrame = origRAF;
    restore();
  });
});

// ---------------------------------------------------------------------------
// Auto-scroll behavior (logic-level tests)
// ---------------------------------------------------------------------------
describe('auto-scroll behavior', () => {
  beforeEach(() => { scrolledUp.value = false; });

  it('auto-scrolls to bottom when scrolledUp is false', () => {
    const el = mockContainer({ scrollTop: 0, scrollHeight: 2000 });
    if (!scrolledUp.value) el.scrollTop = el.scrollHeight;
    expect(el.scrollTop).toBe(2000);
  });

  it('does NOT auto-scroll when scrolledUp is true', () => {
    scrolledUp.value = true;
    const el = mockContainer({ scrollTop: 500, scrollHeight: 2000 });
    if (!scrolledUp.value) el.scrollTop = el.scrollHeight;
    expect(el.scrollTop).toBe(500);
  });

  it('preserves scroll position during more/less toggle when scrolled up', () => {
    scrolledUp.value = true;
    const restore = useMockMO();
    const container = mockContainer({ scrollTop: 400, scrollHeight: 2000 });
    const a = mockDynamicAnchor(container, 300);

    withScrollAnchor(a as any, () => { a._setOffset(500); });

    expect(container.scrollTop).toBe(600); // 400 + 200

    // Auto-scroll does NOT kick in
    if (!scrolledUp.value) container.scrollTop = container.scrollHeight;
    expect(container.scrollTop).toBe(600);

    restore();
  });

  it('preserves scroll position during steps toggle when scrolled up', () => {
    scrolledUp.value = true;
    const restore = useMockMO();
    const container = mockContainer({ scrollTop: 800, scrollHeight: 3000 });
    const a = mockDynamicAnchor(container, 600);

    withScrollAnchor(a as any, () => { a._setOffset(450); });

    expect(container.scrollTop).toBe(650); // 800 - 150

    restore();
  });

  it('scrolledUp resets to false on ThreadView back button', () => {
    scrolledUp.value = true;
    scrolledUp.value = false;
    expect(scrolledUp.value).toBe(false);
  });
});

// ---------------------------------------------------------------------------
// Integration: toggle + auto-scroll interaction
// ---------------------------------------------------------------------------
describe('toggle + auto-scroll interaction', () => {
  beforeEach(() => { scrolledUp.value = false; });

  it('user at bottom → toggle More → stays at bottom via auto-scroll', () => {
    const restore = useMockMO();
    const container = mockContainer({ scrollTop: 500, scrollHeight: 1000, clientHeight: 500 });

    // Verify user is at bottom
    expect(container.scrollTop + container.clientHeight < container.scrollHeight - 80).toBe(false);

    const a = mockDynamicAnchor(container, 300);

    withScrollAnchor(a as any, () => {
      a._setOffset(500);
      container.scrollHeight = 1400;
    });

    expect(container.scrollTop).toBe(700); // 500 + 200

    // Auto-scroll kicks in (scrolledUp is false)
    if (!scrolledUp.value) container.scrollTop = container.scrollHeight;
    expect(container.scrollTop).toBe(1400);

    restore();
  });

  it('user scrolled up → toggle More → position preserved, no auto-scroll', () => {
    scrolledUp.value = true;
    const restore = useMockMO();
    const container = mockContainer({ scrollTop: 200, scrollHeight: 2000, clientHeight: 500 });
    const a = mockDynamicAnchor(container, 150);

    withScrollAnchor(a as any, () => {
      a._setOffset(450);
      container.scrollHeight = 2500;
    });

    expect(container.scrollTop).toBe(500); // 200 + 300

    if (!scrolledUp.value) container.scrollTop = container.scrollHeight;
    expect(container.scrollTop).toBe(500); // preserved

    restore();
  });

  it('user scrolled up → toggle Less → position preserved', () => {
    scrolledUp.value = true;
    const restore = useMockMO();
    const container = mockContainer({ scrollTop: 500, scrollHeight: 2500, clientHeight: 500 });
    const a = mockDynamicAnchor(container, 450);

    withScrollAnchor(a as any, () => {
      a._setOffset(150);
      container.scrollHeight = 2000;
    });

    expect(container.scrollTop).toBe(200); // 500 - 300

    if (!scrolledUp.value) container.scrollTop = container.scrollHeight;
    expect(container.scrollTop).toBe(200); // preserved

    restore();
  });
});

// ---------------------------------------------------------------------------
// Resize: scrolledUp must be recalculated when container dimensions change
// (e.g. coming back from collapsed pane state)
//
// IMPORTANT: Resize can only ESCALATE scrolledUp to true, never clear it.
// Only scroll events (user gesture or programmatic scrollTop) can clear it.
// This prevents layout changes (textarea shrink, idle banner removal) from
// falsely resetting scrolledUp and triggering unwanted auto-scroll.
// ---------------------------------------------------------------------------
describe('scrolledUp recalculation on resize (collapsed → expanded)', () => {
  beforeEach(() => { scrolledUp.value = false; });

  // Resize handler: can only escalate to true, never clear
  function resizeCheck(scrollTop: number, clientHeight: number, scrollHeight: number) {
    if (scrollTop + clientHeight < scrollHeight - 80) {
      scrolledUp.value = true;
    }
  }

  // Scroll handler: can both set and clear
  function scrollCheck(scrollTop: number, clientHeight: number, scrollHeight: number) {
    scrolledUp.value = scrollTop + clientHeight < scrollHeight - 80;
  }

  it('detects scrolledUp after expanding from collapsed state (scrollTop=0, long content)', () => {
    // Collapsed: all dimensions zero, resize doesn't escalate (at bottom)
    resizeCheck(0, 0, 0);
    expect(scrolledUp.value).toBe(false);

    // Expanded: scrollTop stays 0 but content is now tall
    // User is at top of a long document — NOT at bottom — should be scrolledUp=true
    resizeCheck(0, 500, 2000);
    expect(scrolledUp.value).toBe(true);
  });

  it('does not set scrolledUp when content fits in viewport after expand', () => {
    resizeCheck(0, 0, 0);
    expect(scrolledUp.value).toBe(false);

    // Short content — fits in viewport
    resizeCheck(0, 500, 400);
    expect(scrolledUp.value).toBe(false);
  });

  it('does not set scrolledUp when already at bottom after expand', () => {
    resizeCheck(0, 0, 0);
    expect(scrolledUp.value).toBe(false);

    // User is at the bottom of content
    resizeCheck(500, 500, 1000);
    expect(scrolledUp.value).toBe(false);
  });

  it('resize NEVER clears scrolledUp — only scroll events can', () => {
    // User scrolled up
    scrolledUp.value = true;

    // Resize happens (e.g. textarea shrinks, giving more clientHeight)
    // Even though user is now "at bottom" by dimension math, resize must not clear
    resizeCheck(500, 600, 1000);
    expect(scrolledUp.value).toBe(true); // NOT cleared by resize

    // Only a scroll event can clear it (user scrolls to bottom)
    scrollCheck(500, 600, 1000);
    expect(scrolledUp.value).toBe(false); // cleared by scroll
  });
});

// ---------------------------------------------------------------------------
// Auto-scroll dep stability: unrelated SSE events should NOT trigger auto-scroll
// ---------------------------------------------------------------------------
// This tests the fix for: when viewing an idle CC thread while another CC session
// generates events, unrelated threadMap changes caused activeExchanges to produce
// a new array reference, triggering spurious auto-scroll that overrode user scroll.
describe('auto-scroll dep stability', () => {
  beforeEach(() => { scrolledUp.value = false; });

  it('auto-scroll deps should be stable when focused thread data unchanged', () => {
    // Simulate: user viewing idle CC thread, another session generates events.
    // The auto-scroll deps should use eventCount + streamingBuffer (stable)
    // instead of the exchanges array reference (unstable).

    const focusedThreadEvents = new Map<number, any>();
    focusedThreadEvents.set(1, { type: 'MessageReceived', text: 'fix bug' });
    focusedThreadEvents.set(2, { type: 'SessionStarted', session_id: 's1' });
    focusedThreadEvents.set(3, { type: 'CodingAgentIdled', has_changes: true });

    const eventCount = focusedThreadEvents.size;
    const streamingBuffer = '';

    // Stable deps: [eventCount, streamingBuffer]
    const stableDeps = [eventCount, streamingBuffer];

    // Simulate: another thread gets events (threadMap changes, but focused thread unchanged)
    const eventCountAfter = focusedThreadEvents.size; // same — focused thread didn't change
    const streamingBufferAfter = '';

    const stableDepsAfter = [eventCountAfter, streamingBufferAfter];

    // Stable deps should be equal element-by-element
    expect(stableDeps[0]).toBe(stableDepsAfter[0]); // eventCount unchanged
    expect(stableDeps[1]).toBe(stableDepsAfter[1]); // streamingBuffer unchanged

    // Now add an event to the focused thread — deps SHOULD change
    focusedThreadEvents.set(4, { type: 'CodingAgentToolCalled', name: 'Edit', args: {} });
    const eventCountNew = focusedThreadEvents.size;
    expect(eventCountNew).not.toBe(eventCount); // 4 !== 3
  });

  it('auto-scroll should not fire when scrolledUp and deps change', () => {
    scrolledUp.value = true;
    const container = mockContainer({ scrollTop: 200, scrollHeight: 2000 });

    // Simulate auto-scroll effect logic
    if (!scrolledUp.value) container.scrollTop = container.scrollHeight;

    // Position should be preserved
    expect(container.scrollTop).toBe(200);
  });

  it('submitting a reply resets scrolledUp so auto-scroll activates', () => {
    scrolledUp.value = true;
    const container = mockContainer({ scrollTop: 200, scrollHeight: 2000 });

    // Simulate submit(): resets scrolledUp
    scrolledUp.value = false;

    // Auto-scroll now fires because scrolledUp is false
    if (!scrolledUp.value) container.scrollTop = container.scrollHeight;
    expect(container.scrollTop).toBe(2000);
  });

  it('ResizeObserver after submit must NOT re-set scrolledUp (the actual bug)', () => {
    // This test reproduces the exact race condition:
    // 1. User is scrolled up
    // 2. submit() sets scrolledUp = false and collapses textarea
    // 3. ResizeObserver fires on thread-content (textarea collapse made it taller)
    // 4. onResize() sees user is NOT at bottom → re-sets scrolledUp = true
    // 5. useAutoScroll effect sees scrolledUp=true → no scroll
    //
    // The fix: scrollToBottom() must immediately scroll the container so
    // onResize's isAtBottom() check returns true.

    scrolledUp.value = true;
    const container = mockContainer({ scrollTop: 200, scrollHeight: 2000, clientHeight: 500 });

    // Simulate the old broken submit(): just set signal, don't scroll
    scrolledUp.value = false;

    // ResizeObserver fires (textarea collapsed → response area grew)
    // onResize: can only escalate, never clear
    const isAtBottom = container.scrollTop + container.clientHeight >= container.scrollHeight - 80;
    if (!isAtBottom) scrolledUp.value = true;

    // BUG: scrolledUp is back to true — auto-scroll won't fire
    expect(scrolledUp.value).toBe(true); // This is the broken behavior

    // NOW test the fix: scrollToBottom() scrolls container AND sets signal
    scrolledUp.value = false;
    container.scrollTop = container.scrollHeight; // immediate scroll

    // ResizeObserver fires again — but now isAtBottom() returns true
    const isAtBottom2 = container.scrollTop + container.clientHeight >= container.scrollHeight - 80;
    if (!isAtBottom2) scrolledUp.value = true;

    // FIXED: scrolledUp stays false because isAtBottom is true
    expect(scrolledUp.value).toBe(false);

    // useAutoScroll effect fires → scrolls to bottom
    if (!scrolledUp.value) container.scrollTop = container.scrollHeight;
    expect(container.scrollTop).toBe(2000);
  });

  it('pendingUserMessage should trigger auto-scroll when set', () => {
    const container = mockContainer({ scrollTop: 0, scrollHeight: 2000 });
    let pendingMsg: string | null = null;

    // Before: no pending message
    const deps1 = [3, '', pendingMsg];

    // User sends a message — pending message set
    pendingMsg = 'new follow-up message';
    const deps2 = [3, '', pendingMsg];

    // Deps should differ (pendingMsg changed)
    expect(deps1[2]).not.toBe(deps2[2]);

    // Auto-scroll fires (scrolledUp is false)
    if (!scrolledUp.value) container.scrollTop = container.scrollHeight;
    expect(container.scrollTop).toBe(2000);
  });
});

// ---------------------------------------------------------------------------
// Listener re-attachment when DOM element changes (stale ref bug)
// ---------------------------------------------------------------------------
// After engine restart + SSE reconnection, the .thread-content div can be
// recreated by Preact's diffing. The old useAutoScroll attached listeners to
// the old element and never re-ran (deps [ready] didn't change). The fix
// tracks the actual DOM element and reattaches on change.
describe('listener re-attachment on DOM element change', () => {
  beforeEach(() => { scrolledUp.value = false; });

  it('scroll listener should detect element change and reattach', () => {
    // Simulate: old element has listeners, new element does not
    const oldEl = mockContainer({ scrollTop: 0, scrollHeight: 2000, clientHeight: 500 });
    const newEl = mockContainer({ scrollTop: 0, scrollHeight: 2000, clientHeight: 500 });

    // Track which element has the listener
    let listenerEl: ReturnType<typeof mockContainer> | null = null;

    // The fix: check element identity and reattach
    function attachListeners(el: ReturnType<typeof mockContainer> | null, prev: ReturnType<typeof mockContainer> | null) {
      if (el === prev) return prev; // Same element, skip
      // Cleanup old
      if (prev) prev.removeEventListener('scroll', vi.fn());
      // Setup new
      if (el) {
        el.addEventListener('scroll', vi.fn(), { passive: true });
        listenerEl = el;
      }
      return el;
    }

    // Mount: attach to oldEl
    let tracked = attachListeners(oldEl, null);
    expect(listenerEl).toBe(oldEl);

    // Re-render with same element: no change
    tracked = attachListeners(oldEl, tracked);
    expect(listenerEl).toBe(oldEl);

    // Element changes (restart/reconnect): reattach to newEl
    tracked = attachListeners(newEl, tracked);
    expect(listenerEl).toBe(newEl);
  });

  it('scroll detection works on new element after reattachment', () => {
    // New element (created by Preact after reconnection)
    const newEl = mockContainer({ scrollTop: 300, scrollHeight: 2000, clientHeight: 500 });

    // Before fix: listener on old (dead) element, user scrolls new element
    // → scrolledUp never becomes true → button never shows

    // After fix: listener reattached to new element
    // Simulate scroll on new element
    const isAtBottom = newEl.scrollTop + newEl.clientHeight >= newEl.scrollHeight - 80;
    scrolledUp.value = !isAtBottom; // true — user is scrolled up

    expect(scrolledUp.value).toBe(true);
    const className = `scroll-to-bottom${scrolledUp.value ? ' visible' : ''}`;
    expect(className).toBe('scroll-to-bottom visible');
  });
});

// ---------------------------------------------------------------------------
// ResizeObserver suppression during scrollToBottom()
// ---------------------------------------------------------------------------
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
// Continuous rAF loop: scrollToBottom must keep scrolling on every frame
// during the suppression window, not just 2 frames. iOS keyboard animation
// takes 300-400ms with many visualViewport.resize events — the old 2×rAF
// approach missed most of the animation.
// ---------------------------------------------------------------------------
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
// Action buttons (Send / Discard draft / etc.) deliberately do NOT use this
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
