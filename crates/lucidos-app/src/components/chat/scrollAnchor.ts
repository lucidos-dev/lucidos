/** WHERE THE READER IS, said as content rather than as pixels: which turn sits
 *  at the top of the viewport, and the exact offset its own top sat at.
 *
 *  The pane-resize correction in `scrollState.ts` (`recordAnchor`) and the hold
 *  after a restore both measure the line block from here, holding the element
 *  itself. A stored position must outlive the DOM it was measured in. So it
 *  names a child that can be found again, skipping an unidentified one, and the
 *  restore resolves its target by that name.
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
 *  `store/thread-events/exchange-anchors.ts` is where that stamping rule is
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

/** The finest block on the reader's line: the turn at the line, then down
 *  through block children while one straddles the container's top. `path`
 *  holds each step's child index. `relTop` is the block's own top and
 *  `turnRelTop` the turn's, both unrounded. */
export interface LineBlock {
  turn: HTMLElement;
  block: HTMLElement;
  path: number[];
  relTop: number;
  turnRelTop: number;
}

/** The block the reader's line runs through, or null when no turn is at or
 *  above the line. A line in a gap between children stops the descent there,
 *  so the answer is never finer than the content makes it. */
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

/** A reading position expressed against the LINE block inside a named turn:
 *  the turn's id, the child-index path down to the block, and both tops.
 *  `turnRelTop` is the fallback for a path that never resolves. `kinds`
 *  fingerprints the elements along the path (`kindsAlong`). */
export interface LineAnchor {
  eventId: string;
  path: number[];
  kinds: string;
  relTop: number;
  turnRelTop: number;
}

/** The line block as a recordable anchor, or null where only a coarser form
 *  is honest. Three cases answer null:
 *  - the turn at the line has no id, as a *continuation fragment* has none;
 *  - the line runs through no block of it, so the path would be empty;
 *  - the turn is drawn head-clamped, whose missing rows shift every index. */
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

/** The block `anchor` names, or null until it can be measured. That covers a
 *  turn absent or clamped, a missing step of the path, and a block with no box
 *  yet. A path that now runs through other kinds of element is null too. */
export function lineBlockOf(el: HTMLElement, anchor: LineAnchor): HTMLElement | null {
  const turn = anchorTurn(el, anchor);
  if (!turn || turn.hasAttribute?.(HEAD_CLAMPED_ATTR)) return null;
  const walked = walkPath(turn, anchor.path);
  if (!walked || walked.kinds !== anchor.kinds) return null;
  const { block } = walked;
  if (typeof block.getBoundingClientRect !== 'function') return null;
  return block.getBoundingClientRect().height > 0 ? block : null;
}

/** The element `path` leads to from `turn`, and the fingerprint of every
 *  element on the way, or null at a missing step.
 *
 *  An index alone resolves to whatever sits there now. A row inserted ahead
 *  of the reply, as a keep-alive row is, shifts it onto a different element.
 *  Each step's kind is its tag and first class, which a state class leaves
 *  alone, hashed with 32-bit FNV-1a into eight hex digits. */
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

/** The `scrollTop` that puts `anchor`'s block back where it sat, or null while
 *  `lineBlockOf` cannot find it. Same contract as `anchorTargetTop`: null is
 *  the WAIT signal, and both terms are measured now. */
export function lineTargetTop(el: HTMLElement, anchor: LineAnchor): number | null {
  if (typeof el.getBoundingClientRect !== 'function') return null;
  const block = lineBlockOf(el, anchor);
  const rel = block ? blockRelTop(el, block) : null;
  if (rel === null) return null;
  return Math.max(0, Math.round(el.scrollTop + rel - anchor.relTop));
}

/** Where `block` sits now, measured from `el`'s top, or null once it has left
 *  the DOM or lost its box. */
export function blockRelTop(el: HTMLElement, block: HTMLElement): number | null {
  if (block.isConnected === false || typeof block.getBoundingClientRect !== 'function') return null;
  const rect = block.getBoundingClientRect();
  if (rect.height <= 0) return null;
  return rect.top - el.getBoundingClientRect().top;
}

/** The child of `parent` that straddles the viewport line `top`, or null.
 *
 *  The same boundary search as `childAtLine`, since block children are laid
 *  out in document order too. A boxless child (hidden, or `display: contents`)
 *  defers to its nearest boxed sibling before it, so it cannot split the
 *  search. */
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

/** The turn `anchor` names, or null while it is not rendered. */
export function anchorTurn(el: HTMLElement, anchor: Pick<ScrollAnchor, 'eventId'>): HTMLElement | null {
  if (typeof el.querySelector !== 'function') return null;
  return el.querySelector<HTMLElement>(`[${ANCHOR_ATTR}="${CSS.escape(anchor.eventId)}"]`);
}

/** The step row `anchor` names, or null while it is not drawn. */
export function anchorRow(el: HTMLElement, anchor: RowAnchor): HTMLElement | null {
  if (typeof el.querySelector !== 'function') return null;
  return el.querySelector<HTMLElement>(`[${ROW_ATTR}="${CSS.escape(anchor.rowEventId)}"]`);
}

/** The `scrollTop` that puts `anchor`'s row back where it sat, or null while
 *  that row is not drawn. Same contract as `anchorTargetTop`: null is the WAIT
 *  signal, and both terms are measured now. */
export function rowTargetTop(el: HTMLElement, anchor: RowAnchor): number | null {
  if (typeof el.getBoundingClientRect !== 'function') return null;
  const row = anchorRow(el, anchor);
  if (!row || typeof row.getBoundingClientRect !== 'function') return null;
  const rect = row.getBoundingClientRect();
  if (rect.height <= 0) return null;
  return Math.max(0, Math.round(el.scrollTop + (rect.top - el.getBoundingClientRect().top) - anchor.relTop));
}
