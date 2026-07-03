// Shared DOM mock scaffolding for the scroll tests, split across
// scroll-*.test.ts.
import { vi } from 'vitest';

export class MockMutationObserver {
  observe = vi.fn();
  disconnect = vi.fn();
  constructor(_cb?: any) {}
}

export function useMockMO() {
  const orig = globalThis.MutationObserver;
  (globalThis as any).MutationObserver = MockMutationObserver;
  return () => { (globalThis as any).MutationObserver = orig; };
}

/** Minimal mock for setActiveScrollElement — must have getBoundingClientRect
 *  so isElementVisible() works inside scrollToBottom(). scrollToBottom() uses
 *  direct scrollTop assignment which is more reliable than scrollTo(options) on
 *  iOS Safari during viewport transitions. scrollTo() kept for button tests. */
export function mockScrollEl(opts: { scrollTop?: number; scrollHeight?: number; clientHeight?: number }) {
  const el = {
    scrollTop: opts.scrollTop ?? 0,
    scrollHeight: opts.scrollHeight ?? 1000,
    clientHeight: opts.clientHeight ?? 500,
    getBoundingClientRect: () => ({ width: 400, height: 600 }),
    scrollTo(arg: any) {
      if (typeof arg === 'object' && arg.top !== undefined) el.scrollTop = arg.top;
    },
  };
  return el as any;
}

export function mockContainer(opts: {
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

export function mockAnchorInContainer(container: ReturnType<typeof mockContainer>, offsetTop = 200) {
  return {
    offsetTop,
    closest: vi.fn(() => container),
    isConnected: true,
  };
}

export function mockDynamicAnchor(container: ReturnType<typeof mockContainer>, initialOffset: number) {
  let offset = initialOffset;
  return {
    get offsetTop() { return offset; },
    set _offset(v: number) { offset = v; },
    closest: vi.fn(() => container),
    isConnected: true,
    _setOffset(v: number) { offset = v; },
  };
}
