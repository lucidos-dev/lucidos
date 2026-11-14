/** Where a scroll position is saved, and how to drop one.
 *
 *  The data layer resets a view's saved scroll after a save, so these live
 *  apart from `hooks/useScrollMemory.ts`, which restores it. Kept together,
 *  the store pulled the whole restore machinery into the entry chunk
 *  (ADR 0288). */

/** localStorage key for a chat thread's saved scroll offset. */
export function threadScrollKey(threadId: string): string {
  return `lucidos-scroll-thread-${threadId}`;
}

/** localStorage key for ContentPane's per-view scroll offset. ContentPane
 *  reads/writes this directly; the prefix lives here so resetContentScroll
 *  stays in sync. */
export function contentScrollKey(viewKey: string): string {
  return `lucidos-scroll-content-${viewKey}`;
}

/** Drop the saved scroll under `key`, so the next attach restores nothing. */
export function forgetSavedScroll(key: string): void {
  try {
    localStorage.removeItem(key);
  } catch { /* quota or disabled, ignore */ }
}

/** Drop a ContentPane view's saved scroll so the next mount lands at the top
 *  instead of restoring (e.g., after saving a form). */
export function resetContentScroll(viewKey: string): void {
  forgetSavedScroll(contentScrollKey(viewKey));
}

import type { LineAnchor, RowAnchor, ScrollAnchor } from '../components/chat/scrollAnchor';

/** The stored form of a reading position that is the LIVE EDGE rather than a
 *  pixel offset: the reader had a standing follow armed in this thread, and was
 *  ON the edge, when they left it. Deliberately not a number, and deliberately
 *  not a second key beside the offset: a thread opens in exactly one place, so
 *  the two answers share one slot and cannot disagree. */
export const LIVE_EDGE_VALUE = 'live-edge';

/** A marker older builds wrote in front of a place, for a standing follow held
 *  away from the live edge. The follow has no such state any more (ADR 0064),
 *  so it is never written. A stored one is read as the bare place it marks,
 *  with no follow. */
const FOLLOWING_PREFIX = 'following:';

/** The stored form of a reading position that names a TURN:
 *  `anchor:<relTop>:<eventId>`. Why a turn and not a pixel offset, and every
 *  alternative weighed: ADR 0152, docs/adr/.
 *
 *  `relTop` leads, so an id carrying a colon survives the round trip: the parse
 *  splits on the FIRST one. It is EXACT rather than a fallback to "somewhere
 *  near", `relTop` being the pixel offset the anchored turn's top sat at. */
export const ANCHOR_PREFIX = 'anchor:';

/** The stored form of a reading position that names a step ROW:
 *  `row:<relTop>:<rowEventId>`. Laid out like `ANCHOR_PREFIX` and for the same
 *  reasons. The finer of the two, and the one written wherever the turn at the
 *  line has a step row at or above it (`readRowAnchor`). */
export const ROW_PREFIX = 'row:';

/** The stored form of a reading position that names the LINE block:
 *  `line:<relTop>:<turnRelTop>:<eventId>:<path>`. The finest form, written
 *  wherever `readLineAnchor` answers. `path` is child indices joined by `.`.
 *
 *  Both offsets lead and the path trails, so an id carrying a colon survives:
 *  the parse takes the offsets from the first colons and the path from the
 *  last. `turnRelTop` places the turn when the path never resolves. */
export const LINE_PREFIX = 'line:';

/** Where a thread opens. A block, row or TURN the reader was parked on, the
 *  live edge they asked to ride (`LIVE_EDGE_VALUE`), or a bare pixel offset.
 *  `null` for no saved position at all, which `resetOnEmpty` turns into the top.
 *
 *  The offset is what every container that is NOT the transcript records, none
 *  of them being windowed. It is also what a transcript position written by an
 *  older build still reads as.
 *
 *  `live-edge` is the one form that holds a standing follow. A place never
 *  does, so a follow parked away from the edge cannot be recorded. */
export type SavedScroll =
  | { kind: 'offset'; top: number }
  | { kind: 'live-edge' }
  | ({ kind: 'anchor' } & ScrollAnchor)
  | ({ kind: 'row' } & RowAnchor)
  | ({ kind: 'line' } & LineAnchor);

/** Parse a localStorage scroll value. Returns null on missing, invalid or
 *  negative input.
 *
 *  Tolerates a trailing `:<revision>` stamp that older stored positions carry.
 *  `parseFloat` reads the offset out and ignores the rest, so a browser holding
 *  one keeps its position. An `anchor:` value is read first, so that tolerance
 *  never sees one.
 *
 *  A build predating either sentinel reads it as junk and answers null, so a
 *  downgrade opens the thread at the top rather than somewhere wrong. No
 *  migration either way: all that is at stake is where one thread opens once. */
export function parseSavedScroll(raw: string | null): SavedScroll | null {
  if (raw === null || raw === '') return null;
  if (raw === LIVE_EDGE_VALUE) return { kind: 'live-edge' };
  // An older build's marker comes off, and what is left is parsed exactly as
  // an unmarked value is. A marked `live-edge` was never written, and reads as
  // the edge.
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

/** `<relTop>:<eventId>`, split on the FIRST colon so an id may carry one.
 *
 *  `relTop` is matched whole rather than handed to `parseInt`, which reads
 *  `12abc` as 12. A malformed value is one we did not write, and guessing at it
 *  would put the reader somewhere nobody asked for. It is signed: the ordinary
 *  anchor sits at or above the viewport top. */
function parseIdAt(rest: string): { id: string; relTop: number } | null {
  const sep = rest.indexOf(':');
  if (sep < 1) return null;
  const relTop = rest.slice(0, sep);
  const id = rest.slice(sep + 1);
  if (!/^-?\d+$/.test(relTop) || id === '') return null;
  return { id, relTop: Number(relTop) };
}

/** `<relTop>:<turnRelTop>:<eventId>:<path>~<kinds>`. Every part is matched
 *  whole, as `parseIdAt` does, and anything else is a value we did not write. */
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

/** What is RECORDED under `key`, or null for nothing legible.
 *
 *  Storage can THROW rather than answer null, in a browser with it blocked, so
 *  every read in the app goes through this. Exported because ThreadView asks
 *  the same question of the same keys. It has to render the anchored turn
 *  before this hook can put the reader on it. */
export function readSavedScroll(key: string): SavedScroll | null {
  try {
    return parseSavedScroll(localStorage.getItem(key));
  } catch {
    return null;
  }
}
