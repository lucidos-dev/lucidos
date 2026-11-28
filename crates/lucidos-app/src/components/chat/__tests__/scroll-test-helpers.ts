import { vi } from 'vitest';

export function mockStyle(fields: Record<string, string> = { overflow: '' }) {
  const props = new Map<string, string>();
  return Object.assign(fields, {
    getPropertyValue: (name: string) => props.get(name) ?? '',
    setProperty: (name: string, value: string) => { props.set(name, value); },
    removeProperty: (name: string) => { props.delete(name); },
  });
}

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
    style: mockStyle(),
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

export function mockTranscript(opts: {
  ids: string[];
  renderFrom?: number;
  turnHeight?: number;
  clientHeight?: number;
  scrollTop?: number;
  rowsPerTurn?: number;
  blocks?: Record<string, number[]>;
}) {
  let clientHeight = opts.clientHeight ?? 800;
  const listeners: Array<() => void> = [];
  let turnHeight = opts.turnHeight ?? 400;
  let renderFrom = opts.renderFrom ?? 0;
  let top = opts.scrollTop ?? 0;

  const heights = new Map<string, number>();
  const blocks = new Map(Object.entries(opts.blocks ?? {}).map(([id, list]) => [id, [...list]]));
  const sum = (list: number[]) => list.reduce((total, px) => total + px, 0);
  const heightOf = (eventId: string) => {
    const own = blocks.get(eventId);
    return heights.get(eventId) ?? (own ? sum(own)
      : turnHeight - (rowsHidden.get(eventId) ?? 0) * rowHeight());
  };
  const clamped = new Set<string>();
  const rowsHidden = new Map<string, number>();
  const rowsPerTurn = opts.rowsPerTurn ?? 0;
  const rowHeight = () => (rowsPerTurn > 0 ? turnHeight / rowsPerTurn : 0);

  const rendered = () => opts.ids.slice(renderFrom);
  const scrollHeight = () => rendered().reduce((sum, id) => sum + heightOf(id), 0);
  const offsetOf = (indexInWindow: number) =>
    rendered().slice(0, indexInWindow).reduce((sum, id) => sum + heightOf(id), 0);
  const maxTop = () => Math.max(0, scrollHeight() - clientHeight);

  const turnTop = (indexInWindow: number) => CONTAINER_VIEWPORT_TOP + offsetOf(indexInWindow) - top;
  const row = (eventId: string, indexInWindow: number, k: number) => ({
    getAttribute: (name: string) => (name === 'data-row-event' ? `${eventId}-r${k}` : null),
    getBoundingClientRect: () => {
      const height = rowHeight();
      const rectTop = turnTop(indexInWindow) + (k - (rowsHidden.get(eventId) ?? 0)) * height;
      return { top: rectTop, bottom: rectTop + height, height, left: 0, right: 400, width: 400 };
    },
  });
  const drawnRows = (eventId: string, indexInWindow: number) => {
    const out = [];
    for (let k = rowsHidden.get(eventId) ?? 0; k < rowsPerTurn; k++) out.push(row(eventId, indexInWindow, k));
    return out;
  };
  const blockTop = (eventId: string, indexInWindow: number, j: number) =>
    turnTop(indexInWindow) + sum((blocks.get(eventId) ?? []).slice(0, j));
  const generations = new Map<string, number>();
  const classes = new Map<string, Array<string | null>>();
  const block = (eventId: string, indexInWindow: number, j: number) => {
    const generation = generations.get(eventId) ?? 0;
    return {
      children: [],
      get isConnected() { return (generations.get(eventId) ?? 0) === generation; },
      tagName: 'DIV',
      getAttribute: (name: string) => (name === 'class' ? classes.get(eventId)?.[j] ?? null : null),
      getBoundingClientRect: () => {
        const height = blocks.get(eventId)?.[j] ?? 0;
        const rectTop = blockTop(eventId, indexInWindow, j);
        return { top: rectTop, bottom: rectTop + height, height, left: 0, right: 400, width: 400 };
      },
    };
  };
  const turn = (eventId: string, indexInWindow: number) => ({
    get children() { return (blocks.get(eventId) ?? []).map((_, j) => block(eventId, indexInWindow, j)); },
    getAttribute: (name: string) => (name === 'data-event-id' ? eventId : null),
    hasAttribute: (name: string) => name === 'data-head-clamped' && clamped.has(eventId),
    getBoundingClientRect: () => {
      const height = heightOf(eventId);
      const rectTop = turnTop(indexInWindow);
      return { top: rectTop, bottom: rectTop + height, height, left: 0, right: 400, width: 400 };
    },
    querySelectorAll: (selector: string) => (selector === '[data-row-event]' ? drawnRows(eventId, indexInWindow) : []),
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
      const rowMatch = /^\[data-row-event="(.*)-r(\d+)"\]$/.exec(selector);
      if (rowMatch) {
        const index = rendered().indexOf(rowMatch[1]);
        const k = Number(rowMatch[2]);
        if (index < 0 || k < (rowsHidden.get(rowMatch[1]) ?? 0) || k >= rowsPerTurn) return null;
        return row(rowMatch[1], index, k);
      }
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
    growWindowTo: (from: number) => { renderFrom = from; },
    setTurnHeight: (px: number) => { turnHeight = px; },
    setTurnHeightOf: (eventId: string, px: number) => { heights.set(eventId, px); },
    setBlockHeight: (eventId: string, j: number, px: number) => {
      const own = blocks.get(eventId);
      if (own) own[j] = px;
    },
    insertBlock: (eventId: string, j: number, px: number, cls: string) => {
      const own = classes.get(eventId) ?? (blocks.get(eventId) ?? []).map(() => null);
      blocks.get(eventId)?.splice(j, 0, px);
      own.splice(j, 0, cls);
      classes.set(eventId, own);
    },
    replaceBlocks: (eventId: string) => { generations.set(eventId, (generations.get(eventId) ?? 0) + 1); },
    blockRelTop: (eventId: string, j: number) => {
      const index = rendered().indexOf(eventId);
      return blockTop(eventId, index, j) - CONTAINER_VIEWPORT_TOP;
    },
    setHeadClamped: (eventId: string, on: boolean) => { if (on) clamped.add(eventId); else clamped.delete(eventId); },
    setRowsHidden: (eventId: string, k: number) => {
      if (k > 0) { rowsHidden.set(eventId, k); clamped.add(eventId); }
      else { rowsHidden.delete(eventId); clamped.delete(eventId); }
    },
    reclamp: () => { el.scrollTop = top; },
    collapse: () => { turnHeight = 0; clientHeight = 0; top = 0; heights.clear(); },
    reveal: (px: number) => { clientHeight = px; turnHeight = opts.turnHeight ?? 400; },
  };
  return el as any;
}
