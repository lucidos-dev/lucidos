import { describe, it, expect, beforeEach, vi } from 'vitest';

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

describe('awayFromBottom detection', () => {
  beforeEach(() => { awayFromBottom.value = false; });

  function isVisuallyAtBottom(scrollTop: number, clientHeight: number, scrollHeight: number) {
    return scrollTop + clientHeight >= scrollHeight - 2;
  }

  it('is false when exactly at the bottom', () => {
    expect(isVisuallyAtBottom(500, 500, 1000)).toBe(true);
  });

  it('is true after a scroll-up too small to have crossed the old stickiness window', () => {
    expect(isVisuallyAtBottom(480, 500, 1000)).toBe(false);
  });

  it('is false when content fits in viewport (no scroll possible)', () => {
    expect(isVisuallyAtBottom(0, 500, 400)).toBe(true);
    expect(isVisuallyAtBottom(0, 500, 500)).toBe(true);
  });

  it('is true for the small streaming growth the old window deliberately ignored', () => {
    expect(isVisuallyAtBottom(500, 500, 1050)).toBe(false);
  });

  it('a large growth is no different from a small one', () => {
    expect(isVisuallyAtBottom(500, 500, 1500)).toBe(false);
  });
});

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

});

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

    expect(container.scrollTop).toBe(450);
    restore();
  });

  it('adjusts scroll position when anchor moves up', () => {
    const restore = useMockMO();
    const container = mockContainer({ scrollTop: 500 });
    const a = mockDynamicAnchor(container, 400);

    withScrollAnchor(a as any, () => { a._setOffset(300); });

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

describe('global toggles hold the reader on their content', () => {
  it('a reader in history keeps their content across a More toggle', () => {
    const restore = useMockMO();
    const container = mockContainer({ scrollTop: 400, scrollHeight: 2000 });
    const a = mockDynamicAnchor(container, 300);

    withScrollAnchor(a as any, () => { a._setOffset(500); });

    expect(container.scrollTop).toBe(600);
    restore();
  });

  it('a reader in history keeps their content across a steps toggle', () => {
    const restore = useMockMO();
    const container = mockContainer({ scrollTop: 800, scrollHeight: 3000 });
    const a = mockDynamicAnchor(container, 600);

    withScrollAnchor(a as any, () => { a._setOffset(450); });

    expect(container.scrollTop).toBe(650);
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

    expect(container.scrollTop).toBe(700);
    restore();
  });
});
