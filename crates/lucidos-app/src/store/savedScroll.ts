
export function threadScrollKey(threadId: string): string {
  return `lucidos-scroll-thread-${threadId}`;
}

export function contentScrollKey(viewKey: string): string {
  return `lucidos-scroll-content-${viewKey}`;
}

export function forgetSavedScroll(key: string): void {
  try {
    localStorage.removeItem(key);
  } catch {  }
}

export function resetContentScroll(viewKey: string): void {
  forgetSavedScroll(contentScrollKey(viewKey));
}

import type { LineAnchor, RowAnchor, ScrollAnchor } from '../components/chat/scrollAnchor';

export const LIVE_EDGE_VALUE = 'live-edge';

const FOLLOWING_PREFIX = 'following:';

export const ANCHOR_PREFIX = 'anchor:';

export const ROW_PREFIX = 'row:';

export const LINE_PREFIX = 'line:';

export type SavedScroll =
  | { kind: 'offset'; top: number }
  | { kind: 'live-edge' }
  | ({ kind: 'anchor' } & ScrollAnchor)
  | ({ kind: 'row' } & RowAnchor)
  | ({ kind: 'line' } & LineAnchor);

export function parseSavedScroll(raw: string | null): SavedScroll | null {
  if (raw === null || raw === '') return null;
  if (raw === LIVE_EDGE_VALUE) return { kind: 'live-edge' };
  if (raw.startsWith(FOLLOWING_PREFIX)) return parseSavedScroll(raw.slice(FOLLOWING_PREFIX.length));
  if (raw.startsWith(ANCHOR_PREFIX)) {
    const parsed = parseIdAt(raw.slice(ANCHOR_PREFIX.length));
    return parsed && { kind: 'anchor', eventId: parsed.id, relTop: parsed.relTop };
  }
  if (raw.startsWith(ROW_PREFIX)) {
    const parsed = parseIdAt(raw.slice(ROW_PREFIX.length));
    return parsed && { kind: 'row', rowEventId: parsed.id, relTop: parsed.relTop };
  }
  if (raw.startsWith(LINE_PREFIX)) return parseLine(raw.slice(LINE_PREFIX.length));
  const n = Number.parseFloat(raw);
  if (!Number.isFinite(n) || n < 0) return null;
  return { kind: 'offset', top: Math.floor(n) };
}

function parseIdAt(rest: string): { id: string; relTop: number } | null {
  const sep = rest.indexOf(':');
  if (sep < 1) return null;
  const relTop = rest.slice(0, sep);
  const id = rest.slice(sep + 1);
  if (!/^-?\d+$/.test(relTop) || id === '') return null;
  return { id, relTop: Number(relTop) };
}

function parseLine(rest: string): SavedScroll | null {
  const block = parseIdAt(rest);
  const turn = block && parseIdAt(block.id);
  const sep = turn ? turn.id.lastIndexOf(':') : -1;
  if (!block || !turn || sep < 1) return null;
  const tail = /^(\d+(?:\.\d+)*)~([0-9a-f]{8})$/.exec(turn.id.slice(sep + 1));
  if (!tail) return null;
  return {
    kind: 'line',
    eventId: turn.id.slice(0, sep),
    path: tail[1].split('.').map(Number),
    kinds: tail[2],
    relTop: block.relTop,
    turnRelTop: turn.relTop,
  };
}

export function readSavedScroll(key: string): SavedScroll | null {
  try {
    return parseSavedScroll(localStorage.getItem(key));
  } catch {
    return null;
  }
}
