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

/** A WINDOWED transcript: the trailing slice of a thread's turns, each carrying
 *  its `data-event-id`, laid out at a fixed height.
 *
 *  It models the two things a pixel offset cannot survive, which is why the
 *  *reading position* names a turn instead. `renderFrom` moves, standing for the
 *  render window being re-seeded or walked upward. And `scrollTop` CLAMPS on
 *  write, as a real container does, which is what makes the restore's
 *  "the write was clamped, keep waiting" branch reachable.
 *
 *  Deliberately separate from `makeEl` in `hooks/useScrollMemory.test.ts`, which
 *  is a container with no children at all: that one still stands for the content
 *  pane and the thread drawer, neither of which is windowed or names anything. */
export function mockTranscript(opts: {
  ids: string[];
  renderFrom?: number;
  turnHeight?: number;
  clientHeight?: number;
  scrollTop?: number;
}) {
  let clientHeight = opts.clientHeight ?? 800;
  const listeners: Array<() => void> = [];
  let turnHeight = opts.turnHeight ?? 400;
  let renderFrom = opts.renderFrom ?? 0;
  let top = opts.scrollTop ?? 0;

  // Per-turn overrides on top of the shared default, keyed by id so a window
  // change cannot move one onto a different turn.
  const heights = new Map<string, number>();
  const heightOf = (eventId: string) => heights.get(eventId) ?? turnHeight;

  const rendered = () => opts.ids.slice(renderFrom);
  const scrollHeight = () => rendered().reduce((sum, id) => sum + heightOf(id), 0);
  const offsetOf = (indexInWindow: number) =>
    rendered().slice(0, indexInWindow).reduce((sum, id) => sum + heightOf(id), 0);
  const maxTop = () => Math.max(0, scrollHeight() - clientHeight);

  const turn = (eventId: string, indexInWindow: number) => ({
    getAttribute: (name: string) => (name === 'data-event-id' ? eventId : null),
    getBoundingClientRect: () => {
      const height = heightOf(eventId);
      const rectTop = CONTAINER_VIEWPORT_TOP + offsetOf(indexInWindow) - top;
      return { top: rectTop, bottom: rectTop + height, height, left: 0, right: 400, width: 400 };
    },
  });

  const el = {
    get scrollTop() { return top; },
    set scrollTop(v: number) { top = Math.max(0, Math.min(Math.round(v), maxTop())); },
    get scrollHeight() { return scrollHeight(); },
    get clientHeight() { return clientHeight; },
    parentElement: null,
    getBoundingClientRect: () => ({
      top: CONTAINER_VIEWPORT_TOP,
      bottom: CONTAINER_VIEWPORT_TOP + clientHeight,
      height: clientHeight,
      left: 0, right: 400, width: 400,
    }),
    get children() { return rendered().map(turn); },
    querySelector(selector: string) {
      const match = /^\[data-event-id="(.*)"\]$/.exec(selector);
      if (!match) return null;
      const index = rendered().indexOf(match[1]);
      return index < 0 ? null : turn(match[1], index);
    },
    addEventListener: (_t: string, fn: () => void) => { listeners.push(fn); },
    removeEventListener: (_t: string, fn: () => void) => {
      const i = listeners.indexOf(fn);
      if (i >= 0) listeners.splice(i, 1);
    },
    fireScroll: () => { for (const fn of [...listeners]) fn(); },
    /** The render window growing upward, which is the one thing that puts an
     *  out-of-window turn within reach. Leaves `scrollTop` alone, exactly as
     *  prepending content does in a browser. */
    growWindowTo: (from: number) => { renderFrom = from; },
    /** Every turn changing height, which is how the transcript SHRINKS while a
     *  walk is in progress: a live Thinking row folds into its summary. */
    setTurnHeight: (px: number) => { turnHeight = px; },
    /** ONE turn changing height, which is how the transcript shrinks BELOW an
     *  anchor between two opens. The live Thinking row is derived, so the last
     *  turn loses it the moment that turn finishes (ADR 0066). */
    setTurnHeightOf: (eventId: string, px: number) => { heights.set(eventId, px); },
    /** A transcript nobody can MEASURE, which a collapsed desktop split gives:
     *  every rect reads all-zero while the children are still there. The
     *  browser clamps `scrollTop` to the vanished range, and that clamp is what
     *  fires the scroll event this models. */
    collapse: () => { turnHeight = 0; clientHeight = 0; top = 0; heights.clear(); },
    /** The pane coming back, which is what the restore's wait is held open
     *  for. `turnHeight` returns to the shared default it started at. */
    reveal: (px: number) => { clientHeight = px; turnHeight = opts.turnHeight ?? 400; },
  };
  return el as any;
}
