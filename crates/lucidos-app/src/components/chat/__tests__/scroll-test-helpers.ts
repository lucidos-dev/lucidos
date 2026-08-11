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

/** The transcript's own viewport position. Arbitrary and non-zero on purpose:
 *  `withScrollAnchor` measures the anchor RELATIVE to this box, so a helper that
 *  pinned it at 0 would let a missing subtraction pass. */
const CONTAINER_VIEWPORT_TOP = 120;

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
    getBoundingClientRect: () => ({
      top: CONTAINER_VIEWPORT_TOP,
      bottom: CONTAINER_VIEWPORT_TOP + el.clientHeight,
      height: el.clientHeight,
      left: 0, right: 400, width: 400,
    }),
  };
  return el;
}

/** A turn inside `container`, `offsetTop` from the top of its scrolled content.
 *
 *  It answers `getBoundingClientRect` as the real thing does, derived from that
 *  offset and the container's current `scrollTop`, because `withScrollAnchor`
 *  measures through rects rather than `offsetTop` (see `contentOffsetTop`: the
 *  platform rounds `offsetTop` to a whole pixel, and a correction built from two
 *  rounded reads twitches the transcript by the difference). Keeping `offsetTop`
 *  on the double as the way a test MOVES the turn keeps every case readable, and
 *  the rect stays derived from it so the two can never disagree. */
function anchorIn(container: ReturnType<typeof mockContainer>, read: () => number) {
  return {
    get offsetTop() { return read(); },
    closest: vi.fn(() => container),
    isConnected: true,
    getBoundingClientRect: () => {
      const top = container.getBoundingClientRect().top + read() - container.scrollTop;
      return { top, bottom: top, height: 0, left: 0, right: 400, width: 400 };
    },
  };
}

export function mockAnchorInContainer(container: ReturnType<typeof mockContainer>, offsetTop = 200) {
  return anchorIn(container, () => offsetTop);
}

export function mockDynamicAnchor(container: ReturnType<typeof mockContainer>, initialOffset: number) {
  let offset = initialOffset;
  return Object.assign(anchorIn(container, () => offset), {
    _setOffset(v: number) { offset = v; },
  });
}
