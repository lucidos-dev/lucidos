
export const ANCHOR_ATTR = 'data-event-id';

export const HEAD_CLAMPED_ATTR = 'data-head-clamped';

export const ROW_ATTR = 'data-row-event';

export interface RowAnchor {
  rowEventId: string;
  relTop: number;
}

export interface ScrollAnchor {
  eventId: string;
  relTop: number;
}

export function readScrollAnchor(el: HTMLElement): ScrollAnchor | null {
  const line = childAtLine(el);
  if (!line) return null;
  const { kids, last, lastRel, relTopOf } = line;
  for (let i = last; i >= 0; i--) {
    const named = namedAt(kids, i, i === last ? lastRel : relTopOf(i));
    if (named) return named;
  }
  for (let i = last + 1; i < kids.length; i++) {
    const named = namedAt(kids, i, relTopOf(i));
    if (named) return named;
  }
  return null;
}

export function readRowAnchor(el: HTMLElement): RowAnchor | null {
  const line = childAtLine(el);
  if (!line || line.last < 0) return null;
  const turn = line.kids[line.last] as HTMLElement;
  if (typeof turn.querySelectorAll !== 'function') return null;
  const rows = turn.querySelectorAll<HTMLElement>(`[${ROW_ATTR}]`);
  let lo = 0;
  let hi = rows.length - 1;
  let found: RowAnchor | null = null;
  while (lo <= hi) {
    const mid = (lo + hi) >> 1;
    const rect = rows[mid].getBoundingClientRect();
    const rel = rect.top - line.top;
    if (rect.height > 0 && rel <= 0) {
      const rowEventId = rows[mid].getAttribute(ROW_ATTR);
      if (rowEventId) found = { rowEventId, relTop: Math.round(rel) };
      lo = mid + 1;
    } else {
      hi = mid - 1;
    }
  }
  return found;
}

export interface LineBlock {
  turn: HTMLElement;
  block: HTMLElement;
  path: number[];
  relTop: number;
  turnRelTop: number;
}

export function readLineBlock(el: HTMLElement): LineBlock | null {
  const line = childAtLine(el);
  if (!line || line.last < 0 || line.lastRel === null) return null;
  const turn = line.kids[line.last] as HTMLElement;
  let block = turn;
  let relTop = line.lastRel;
  const path: number[] = [];
  for (let next = straddlerOf(block, line.top); next; next = straddlerOf(block, line.top)) {
    path.push(next.index);
    block = next.block;
    relTop = next.relTop;
  }
  return { turn, block, path, relTop, turnRelTop: line.lastRel };
}

export interface LineAnchor {
  eventId: string;
  path: number[];
  kinds: string;
  relTop: number;
  turnRelTop: number;
}

export function readLineAnchor(el: HTMLElement): LineAnchor | null {
  const line = readLineBlock(el);
  if (!line || line.path.length === 0) return null;
  const eventId = line.turn.getAttribute?.(ANCHOR_ATTR);
  if (!eventId || line.turn.hasAttribute?.(HEAD_CLAMPED_ATTR)) return null;
  const walked = walkPath(line.turn, line.path);
  if (!walked) return null;
  return {
    eventId,
    path: line.path,
    kinds: walked.kinds,
    relTop: Math.round(line.relTop),
    turnRelTop: Math.round(line.turnRelTop),
  };
}

export function lineBlockOf(el: HTMLElement, anchor: LineAnchor): HTMLElement | null {
  const turn = anchorTurn(el, anchor);
  if (!turn || turn.hasAttribute?.(HEAD_CLAMPED_ATTR)) return null;
  const walked = walkPath(turn, anchor.path);
  if (!walked || walked.kinds !== anchor.kinds) return null;
  const { block } = walked;
  if (typeof block.getBoundingClientRect !== 'function') return null;
  return block.getBoundingClientRect().height > 0 ? block : null;
}

function walkPath(turn: HTMLElement, path: number[]): { block: HTMLElement; kinds: string } | null {
  let block = turn;
  let hash = 0x811c9dc5;
  for (const index of path) {
    const next = block.children?.[index] as HTMLElement | undefined;
    if (!next) return null;
    block = next;
    const kind = `${block.tagName ?? ''}.${(block.getAttribute?.('class') ?? '').split(' ')[0]}/`;
    for (let i = 0; i < kind.length; i++) hash = Math.imul(hash ^ kind.charCodeAt(i), 0x01000193);
  }
  return { block, kinds: (hash >>> 0).toString(16).padStart(8, '0') };
}

export function lineTargetTop(el: HTMLElement, anchor: LineAnchor): number | null {
  if (typeof el.getBoundingClientRect !== 'function') return null;
  const block = lineBlockOf(el, anchor);
  const rel = block ? blockRelTop(el, block) : null;
  if (rel === null) return null;
  return Math.max(0, Math.round(el.scrollTop + rel - anchor.relTop));
}

export function blockRelTop(el: HTMLElement, block: HTMLElement): number | null {
  if (block.isConnected === false || typeof block.getBoundingClientRect !== 'function') return null;
  const rect = block.getBoundingClientRect();
  if (rect.height <= 0) return null;
  return rect.top - el.getBoundingClientRect().top;
}

function straddlerOf(parent: HTMLElement, top: number): { block: HTMLElement; index: number; relTop: number } | null {
  const kids = parent.children;
  if (!kids || kids.length === 0) return null;
  const rectOf = (i: number) => {
    const kid = kids[i] as HTMLElement;
    if (typeof kid.getBoundingClientRect !== 'function') return null;
    const rect = kid.getBoundingClientRect();
    return rect.height > 0 ? rect : null;
  };
  let lo = 0;
  let hi = kids.length - 1;
  let last = -1;
  while (lo <= hi) {
    const mid = (lo + hi) >> 1;
    let boxed = mid;
    while (boxed >= lo && !rectOf(boxed)) boxed--;
    if (boxed < lo) { lo = mid + 1; continue; }
    if (rectOf(boxed)!.top - top <= 0) { last = boxed; lo = mid + 1; } else { hi = boxed - 1; }
  }
  if (last < 0) return null;
  const rect = rectOf(last)!;
  if (rect.top + rect.height <= top) return null;
  return { block: kids[last] as HTMLElement, index: last, relTop: rect.top - top };
}

function childAtLine(el: HTMLElement): {
  kids: HTMLCollection;
  last: number;
  lastRel: number | null;
  top: number;
  relTopOf: (i: number) => number | null;
} | null {
  const feed = typeof el.querySelector === 'function' ? el.querySelector('.thread-feed') : null;
  const kids = (feed ?? el).children;
  if (!kids || typeof el.getBoundingClientRect !== 'function') return null;
  const top = el.getBoundingClientRect().top;
  const relTopOf = (i: number): number | null => {
    const kid = kids[i] as HTMLElement | undefined;
    if (!kid || typeof kid.getBoundingClientRect !== 'function') return null;
    const rect = kid.getBoundingClientRect();
    return rect.height <= 0 ? null : rect.top - top;
  };
  let lo = 0;
  let hi = kids.length - 1;
  let last = -1;
  let lastRel: number | null = null;
  while (lo <= hi) {
    const mid = (lo + hi) >> 1;
    const rel = relTopOf(mid);
    if (rel !== null && rel <= 0) { last = mid; lastRel = rel; lo = mid + 1; } else { hi = mid - 1; }
  }
  return { kids, last, lastRel, top, relTopOf };
}

function namedAt(kids: HTMLCollection, i: number, relTop: number | null): ScrollAnchor | null {
  if (relTop === null) return null;
  const eventId = (kids[i] as HTMLElement).getAttribute?.(ANCHOR_ATTR);
  return eventId ? { eventId, relTop: Math.round(relTop) } : null;
}

export function anchorTargetTop(el: HTMLElement, anchor: ScrollAnchor): number | null {
  if (typeof el.getBoundingClientRect !== 'function') return null;
  const target = anchorTurn(el, anchor);
  if (!target || typeof target.getBoundingClientRect !== 'function') return null;
  const rect = target.getBoundingClientRect();
  if (rect.height <= 0) return null;
  return Math.max(0, Math.round(el.scrollTop + (rect.top - el.getBoundingClientRect().top) - anchor.relTop));
}

export function anchorTurnIsClamped(el: HTMLElement, anchor: ScrollAnchor): boolean {
  return anchorTurn(el, anchor)?.hasAttribute?.(HEAD_CLAMPED_ATTR) === true;
}

export function anchorTurn(el: HTMLElement, anchor: Pick<ScrollAnchor, 'eventId'>): HTMLElement | null {
  if (typeof el.querySelector !== 'function') return null;
  return el.querySelector<HTMLElement>(`[${ANCHOR_ATTR}="${CSS.escape(anchor.eventId)}"]`);
}

export function anchorRow(el: HTMLElement, anchor: RowAnchor): HTMLElement | null {
  if (typeof el.querySelector !== 'function') return null;
  return el.querySelector<HTMLElement>(`[${ROW_ATTR}="${CSS.escape(anchor.rowEventId)}"]`);
}

export function rowTargetTop(el: HTMLElement, anchor: RowAnchor): number | null {
  if (typeof el.getBoundingClientRect !== 'function') return null;
  const row = anchorRow(el, anchor);
  if (!row || typeof row.getBoundingClientRect !== 'function') return null;
  const rect = row.getBoundingClientRect();
  if (rect.height <= 0) return null;
  return Math.max(0, Math.round(el.scrollTop + (rect.top - el.getBoundingClientRect().top) - anchor.relTop));
}
