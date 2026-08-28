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
 *  "which turn", so a second marker could only disagree. */
const ANCHOR_ATTR = 'data-event-id';

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
 *  named.
 *
 *  BINARY SEARCH, because this runs on every scroll event of the transcript. A
 *  linear scan is unbounded exactly where the transcript is largest: the chevron
 *  and a deep link each render the thread WHOLE and leave the reader at the top.
 *  From there a scan in either direction measures every turn, at up to 120
 *  events a second.
 *
 *  IT RESTS ON ONE INVARIANT: no BOXLESS child sits between two turns. Turns are
 *  laid out in document order, so their tops rise along the list and the
 *  boundary is findable in `log n` rect reads. A boxless child breaks that
 *  order, and one in the middle can make the search answer for an earlier turn.
 *  Today the transcript's only one is the mobile title row at index 0, where the
 *  scan out below recovers. Its other non-turn children (the empty state, a
 *  queued-message group) all carry boxes.
 *
 *  A reader ABOVE the first named turn still gets an anchor, that turn with a
 *  positive `relTop`. Answering null there would record the top of a re-seeded
 *  window as "no position", which is not where they were. */
export function readScrollAnchor(el: HTMLElement): ScrollAnchor | null {
  const kids = el.children;
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
  // The last child at or above the line, or -1 when the reader is above them
  // all. A boxless child answers null and reads as BELOW, which is the invariant
  // above: harmless at the head, wrong in the middle. `lastRel` carries the
  // boundary's measurement out, so the scan does not pay for it twice.
  let lo = 0;
  let hi = kids.length - 1;
  let last = -1;
  let lastRel: number | null = null;
  while (lo <= hi) {
    const mid = (lo + hi) >> 1;
    const rel = relTopOf(mid);
    if (rel !== null && rel <= 0) { last = mid; lastRel = rel; lo = mid + 1; } else { hi = mid - 1; }
  }
  // Then out from the boundary to the nearest child that can be NAMED. Back
  // from `last` is the reader's own turn. Forward from it is the earliest turn,
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
  if (typeof el.querySelector !== 'function' || typeof el.getBoundingClientRect !== 'function') return null;
  const target = el.querySelector<HTMLElement>(`[${ANCHOR_ATTR}="${CSS.escape(anchor.eventId)}"]`);
  if (!target || typeof target.getBoundingClientRect !== 'function') return null;
  const rect = target.getBoundingClientRect();
  if (rect.height <= 0) return null;
  return Math.max(0, Math.round(el.scrollTop + (rect.top - el.getBoundingClientRect().top) - anchor.relTop));
}
