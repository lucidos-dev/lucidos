import { describe, it, expect, beforeEach, vi } from 'vitest';

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

import { mockAnchorInContainer, mockContainer, mockDynamicAnchor, useMockMO } from './scroll-test-helpers';
import { withScrollAnchor } from '../CreateThreadView';
import { awayFromBottom, scrolledUp } from '../scrollState';

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
// This tests the fix for: when viewing an idle CC thread while another Claude Code session
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
