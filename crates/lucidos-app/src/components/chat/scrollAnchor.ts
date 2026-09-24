/** WHERE THE READER IS, said as content rather than as pixels: which turn sits
 *  at the top of the viewport, and the exact offset its own top sat at.
 *
 *  It is the durable twin of `recordAnchor` and `restoreAfterReflow` in
 *  `scrollState.ts`. Those take the same two measurements to hold a reader still
 *  across a pane resize. Two differences here, both forced by the position
 *  outliving the DOM it was measured in. The anchor must name a child that can
 *  be found again, so an unidentified one is skipped. And the restore resolves
 *  its target by that name, where the correction still holds the element itself.
 *
 *  A LEAF: this module reaches for no store, no signal and no storage. The
 *  measurement is here and the codec is in `hooks/useScrollMemory.ts`, so
 *  neither half can grow a dependency on the other's world. ADR 0152 (docs/adr/)
 *  carries why a reading position names a turn at all.
 *
 *  Every DOM call is guarded. This runs in the DOM-free unit environment, and
 *  against the fake elements the scroll-memory tests drive the whole lifecycle
 *  with. */

/** The attribute an anchorable child is found by. A `.chat-exchange` root
 *  carries its turn's starter event id, and `stampedEventIds` in
 *  `store/thread-events/exchange-render.ts` is where that stamping rule is
 *  declared. Sharing the attribute with the deep link is deliberate: both ask
 *  "which turn", so a second marker could only disagree.
 *
 *  Exported because the codec in `hooks/useScrollMemory.ts` watches it. A
 *  backfill fold gives a fragment turn this attribute in place. The answer
 *  below then changes with nothing else moving. */
export const ANCHOR_ATTR = 'data-event-id';

/** Marks a turn the render window draws with its leading rows clamped off.
 *  Its top edge is real, but the content under it is short until the window
 *  walks the rest in. `ChatExchange` stamps it. */
export const HEAD_CLAMPED_ATTR = 'data-head-clamped';

/** The attribute a step ROW is found by: the event id of the tool call it
 *  shows (`call_event_id`), else of the result that settled it
 *  (`result_event_id`). `InlineStep` stamps it.
 *
 *  Finer than a turn, which on a coding-agent thread can hold hundreds of
 *  rows. And stable where a turn's is not: an event id survives the fold, a
 *  *continuation fragment* and the page boundary alike. */
export const ROW_ATTR = 'data-row-event';

/** A reading position expressed against a step row: its event id, and the
 *  offset its top sat at, measured from the container's top. */
export interface RowAnchor {
  rowEventId: string;
  relTop: number;
}

/** A reading position expressed against a turn.
 *
 *  `relTop` is that turn's top, measured from the container's top. It is at or
 *  below zero for the ordinary anchor, and above it only for a reader parked
 *  over the very first turn. */
export interface ScrollAnchor {
  eventId: string;
  relTop: number;
}

/** The turn the reader is parked on, or null when nothing on screen can be
 *  named. Found off the same boundary search as the row (`childAtLine`).
 *
 *  A reader ABOVE the first named turn still gets an anchor, that turn with a
 *  positive `relTop`. Answering null there would record the top of a re-seeded
 *  window as "no position", which is not where they were. */
export function readScrollAnchor(el: HTMLElement): ScrollAnchor | null {
  const line = childAtLine(el);
  if (!line) return null;
  const { kids, last, lastRel, relTopOf } = line;
  // Out from the boundary to the nearest child that can be NAMED. Back from
  // `last` is the reader's own turn. Forward from it is the earliest turn,
  // which is the answer for a reader above them all.
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

/** The step row the reader is parked on, or null when the turn at the line
 *  has no step row at or above it.
 *
 *  Searched inside the turn at the line only. So the cost is one turn's rows,
 *  and the answer is never further away than the turn anchor. That turn needs
 *  no id: a *continuation fragment* has none, and its rows do.
 *
 *  Measured from the ROW, so a turn drawn with its head clamped off measures
 *  the same as the whole turn. A turn anchor taken there measured from the
 *  clamped top, and landed the reader above their place on the next open. */
export function readRowAnchor(el: HTMLElement): RowAnchor | null {
  const line = childAtLine(el);
  if (!line || line.last < 0) return null;
  const turn = line.kids[line.last] as HTMLElement;
  if (typeof turn.querySelectorAll !== 'function') return null;
  const rows = turn.querySelectorAll<HTMLElement>(`[${ROW_ATTR}]`);
  // Rows are laid out in document order, so the same boundary search applies.
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

/** The feed's children, and the boundary: the last child at or above the
 *  container's top, or -1 when the reader is above them all.
 *
 *  BINARY SEARCH, because this runs on every scroll event of the transcript. A
 *  linear scan is unbounded exactly where the transcript is largest: the chevron
 *  and a deep link each render the thread WHOLE and leave the reader at the top.
 *
 *  IT RESTS ON ONE INVARIANT: no BOXLESS child sits between two turns. Turns are
 *  laid out in document order, so their tops rise along the list. A boxless
 *  child reads as below the line, harmless at the head and wrong in the middle.
 *  Today the only one is the mobile title row at index 0. */
function childAtLine(el: HTMLElement): {
  kids: HTMLCollection;
  last: number;
  lastRel: number | null;
  top: number;
  relTopOf: (i: number) => number | null;
} | null {
  // THE TURNS, wherever they are parented. The transcript keeps them in a feed
  // box of its own (`.thread-feed` in styles/chat/input-messages.css), whose
  // wrapper carries no id. `top` stays the SCROLLER's, since `relTop` is
  // measured from the scrollport.
  const feed = typeof el.querySelector === 'function' ? el.querySelector('.thread-feed') : null;
  const kids = (feed ?? el).children;
  if (!kids || typeof el.getBoundingClientRect !== 'function') return null;
  const top = el.getBoundingClientRect().top;
  const relTopOf = (i: number): number | null => {
    const kid = kids[i] as HTMLElement | undefined;
    if (!kid || typeof kid.getBoundingClientRect !== 'function') return null;
    const rect = kid.getBoundingClientRect();
    // Boxless children are skipped: on desktop the mobile title row reports an
    // all-zero rect, which would otherwise read as sitting exactly on the line.
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

/** The anchor a child yields, or null when it carries no id or has no box. */
function namedAt(kids: HTMLCollection, i: number, relTop: number | null): ScrollAnchor | null {
  if (relTop === null) return null;
  const eventId = (kids[i] as HTMLElement).getAttribute?.(ANCHOR_ATTR);
  return eventId ? { eventId, relTop: Math.round(relTop) } : null;
}

/** The `scrollTop` that puts `anchor`'s turn back where it sat, or null when
 *  that turn is not rendered yet.
 *
 *  Null is the WAIT signal rather than a failure. The transcript is windowed, so
 *  an anchored turn above the window is absent until ThreadView has walked the
 *  window up to it. The restore retries on every mutation until this answers.
 *
 *  Measured, never accumulated. Both terms are read now, so the answer is
 *  immune to a browser clamp having moved `scrollTop` since the record was
 *  taken. Clamped at zero on the way out, which is where a positive `relTop`
 *  taken over the first turn lands. */
export function anchorTargetTop(el: HTMLElement, anchor: ScrollAnchor): number | null {
  if (typeof el.getBoundingClientRect !== 'function') return null;
  const target = anchorTurn(el, anchor);
  if (!target || typeof target.getBoundingClientRect !== 'function') return null;
  const rect = target.getBoundingClientRect();
  if (rect.height <= 0) return null;
  return Math.max(0, Math.round(el.scrollTop + (rect.top - el.getBoundingClientRect().top) - anchor.relTop));
}

/** Is the turn `anchor` names drawn with its head clamped off? False for a
 *  turn that is whole, and for one that is not rendered at all. */
export function anchorTurnIsClamped(el: HTMLElement, anchor: ScrollAnchor): boolean {
  return anchorTurn(el, anchor)?.hasAttribute?.(HEAD_CLAMPED_ATTR) === true;
}

function anchorTurn(el: HTMLElement, anchor: ScrollAnchor): HTMLElement | null {
  if (typeof el.querySelector !== 'function') return null;
  return el.querySelector<HTMLElement>(`[${ANCHOR_ATTR}="${CSS.escape(anchor.eventId)}"]`);
}

/** The `scrollTop` that puts `anchor`'s row back where it sat, or null while
 *  that row is not drawn. Same contract as `anchorTargetTop`: null is the WAIT
 *  signal, and both terms are measured now. */
export function rowTargetTop(el: HTMLElement, anchor: RowAnchor): number | null {
  if (typeof el.getBoundingClientRect !== 'function' || typeof el.querySelector !== 'function') return null;
  const row = el.querySelector<HTMLElement>(`[${ROW_ATTR}="${CSS.escape(anchor.rowEventId)}"]`);
  if (!row || typeof row.getBoundingClientRect !== 'function') return null;
  const rect = row.getBoundingClientRect();
  if (rect.height <= 0) return null;
  return Math.max(0, Math.round(el.scrollTop + (rect.top - el.getBoundingClientRect().top) - anchor.relTop));
}
