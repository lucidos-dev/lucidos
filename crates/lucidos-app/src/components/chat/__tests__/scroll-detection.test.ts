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
import { awayFromBottom } from '../scrollState';

// ---------------------------------------------------------------------------
// awayFromBottom is the ONE position threshold the transcript has left, and it
// drives one thing: whether the down chevron is offered.
//
// There used to be a second, `scrolledUp`, with an 80px stickiness window:
// inside it a reader still counted as riding the live edge and content growth
// re-pinned them to the bottom. Nothing pins now, so the window has nothing to
// decide and the signal is gone. The cases below are its cases, re-pointed at
// the threshold that survived.
// ---------------------------------------------------------------------------
describe('awayFromBottom detection', () => {
  beforeEach(() => { awayFromBottom.value = false; });

  function isVisuallyAtBottom(scrollTop: number, clientHeight: number, scrollHeight: number) {
    return scrollTop + clientHeight >= scrollHeight - 2;
  }

  it('is false when exactly at the bottom', () => {
    expect(isVisuallyAtBottom(500, 500, 1000)).toBe(true);
  });

  it('is true after a scroll-up too small to have crossed the old stickiness window', () => {
    // 20px from the bottom. The retired `scrolledUp` would still have read this
    // as "riding the live edge" and pinned them back down on the next chunk.
    expect(isVisuallyAtBottom(480, 500, 1000)).toBe(false);
  });

  it('is false when content fits in viewport (no scroll possible)', () => {
    expect(isVisuallyAtBottom(0, 500, 400)).toBe(true);
    expect(isVisuallyAtBottom(0, 500, 500)).toBe(true);
  });

  it('is true for the small streaming growth the old window deliberately ignored', () => {
    // A token adds ~50px under a reader at the bottom of 1000px of content.
    // The old resize rule left the chevron hidden here, because the layout
    // effect was about to snap them back down anyway. Nothing snaps, so the
    // reader IS below the fold and the chevron is how they follow.
    expect(isVisuallyAtBottom(500, 500, 1050)).toBe(false);
  });

  it('a large growth is no different from a small one', () => {
    expect(isVisuallyAtBottom(500, 500, 1500)).toBe(false);
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

  // Behavioural coverage of the chevron itself lives in
  // scroll-reader-owns-position.test.ts and scroll-to-top.test.ts.
});

// ---------------------------------------------------------------------------
// withScrollAnchor — scroll position preservation
//
// The transcript's ONE remaining right to move itself: holding the reader on
// the same content while a global toggle (More/Less, Show steps) changes the
// height of everything around them.
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
// Toggling More / Show steps: the anchor is the WHOLE behaviour.
//
// These cases used to come in pairs, one for a reader who was parked (position
// preserved) and one for a reader at the bottom (auto-scroll re-pinned them to
// the new bottom, overriding the anchor it had just applied). The second half
// of each pair is inverted: the anchor is the answer for everyone.
// ---------------------------------------------------------------------------
describe('global toggles hold the reader on their content', () => {
  it('a reader in history keeps their content across a More toggle', () => {
    const restore = useMockMO();
    const container = mockContainer({ scrollTop: 400, scrollHeight: 2000 });
    const a = mockDynamicAnchor(container, 300);

    withScrollAnchor(a as any, () => { a._setOffset(500); });

    expect(container.scrollTop).toBe(600); // 400 + 200
    restore();
  });

  it('a reader in history keeps their content across a steps toggle', () => {
    const restore = useMockMO();
    const container = mockContainer({ scrollTop: 800, scrollHeight: 3000 });
    const a = mockDynamicAnchor(container, 600);

    withScrollAnchor(a as any, () => { a._setOffset(450); });

    expect(container.scrollTop).toBe(650); // 800 - 150
    restore();
  });

  it('a reader AT THE BOTTOM keeps their content too, rather than being re-pinned', () => {
    const restore = useMockMO();
    const container = mockContainer({ scrollTop: 500, scrollHeight: 1000, clientHeight: 500 });
    const a = mockDynamicAnchor(container, 300);

    withScrollAnchor(a as any, () => {
      a._setOffset(500);
      container.scrollHeight = 1400;
    });

    // The anchor correction, and then nothing. This case used to continue
    // `if (!scrolledUp.value) container.scrollTop = container.scrollHeight`,
    // landing them at 1400 and hiding the very growth they had asked to see.
    expect(container.scrollTop).toBe(700); // 500 + 200
    restore();
  });
});
