import { isIOS } from './platform';
import { hasPendingEventScroll } from '../components/chat/scrollState';
import { isUserScrolling } from './scrollActivity';

/** Force iOS Safari / WKWebView to repaint a compositor layer whose backing
 *  texture it has blanked: the content is in the DOM but renders black. WebKit
 *  stops committing the layer tree, so the layer freezes on a stale texture
 *  until a scroll, rotation or layout change forces a fresh commit. The iOS
 *  PWA is Safari's own WKWebView, so a frontend repaint is the only recovery
 *  lever on that surface.
 *
 *  Three additive nudges across two animation frames, applied in frame one and
 *  restored in frame two, so the intermediate state is genuinely painted:
 *    1. A sub-pixel `translateZ` round-trip, the only lever for a
 *       non-scrollable element.
 *    2. A forced layout read, which flushes the pending layout so the nudged
 *       frame paints instead of being swallowed.
 *    3. A 1px `scrollTop` nudge, taken from the LIVE scrollTop at nudge time
 *       so streaming growth cannot make it stale. It rides out on a matching
 *       `translateY`, so the layer gets the offset change and the reader sees
 *       nothing move (see `nudgedTransform`).
 *
 *  No-op off iOS and for detached nodes. Returns a cleanup that cancels the
 *  pending frames AND restores both baselines, so a caller can hand it
 *  straight back from a `useEffect`. */

/** Marks a DIRECT child the scroll does NOT move: one pinned to the scrollport
 *  by `position: sticky`. The mobile thread title bar is the only one today.
 *  A role marker rather than a class, so this shared recovery lever never names
 *  a component.
 *
 *  Direct is the contract, not an accident of today's tree: only the scroller's
 *  own children are scanned, so nesting the row one level deeper would stop the
 *  publish silently. Pinned by styles/__tests__/sticky-title-nudge-counter-guard.test.ts. */
export const SCROLLER_PINNED_ATTR = 'data-scroller-pinned';

/** How far the container's compensation moved a pinned child, in px. The child
 *  subtracts it in its own `transform`. See `publishPinnedShift`. */
export const PINNED_SHIFT_PROP = '--repaint-nudge-shift';

/** Per-element in-flight toggle. Tracks the scheduled frames and the transform
 *  baseline (captured once on the first call of a burst) plus this toggle's own
 *  scroll nudge, so a superseding call can undo a partial nudge cleanly. */
interface InFlightToggle {
  raf1: number;
  raf2: number | undefined;
  prev: string;
  /** "Should we nudge `scrollTop`?", decided ONCE at burst start and reused on
   *  every superseding call. So rAF1 (nudge) and rAF2 (restore) always agree
   *  even though `hasPendingEventScroll()` can flip mid-burst. Never
   *  nudge-then-skip-restore or skip-then-restore. */
  nudgeScroll: boolean;
  /** The LIVE scrollTop this toggle's raf1 nudged FROM, and the value it nudged
   *  TO. Captured IN raf1, not at call time, so streaming growth between the
   *  call and the nudge cannot make the nudge stale. `undefined` until raf1 has
   *  nudged, so `restoreNudge` is a no-op until then. */
  restoreTop: number | undefined;
  nudgedTop: number | undefined;
  /** The pinned children this toggle published its shift onto, so the clear
   *  cannot miss an element the publish hit. Empty until rAF1 has published. */
  pinned: HTMLElement[];
}
const inFlight = new WeakMap<object, InFlightToggle>();

/** `performance.now()` of the most recent `scrollTop` write made by the nudge. */
let lastNudgeAt = -Infinity;

/** How long after a nudge write its `scroll` event is still expected to arrive.
 *  A `scrollTop` write does not dispatch `scroll` synchronously: the event is
 *  queued and fired in the next frame's "run the scroll steps", BEFORE that
 *  frame's rAF callbacks. So the window has to outlive the write by a frame,
 *  which rules out clearing a flag in the restoring rAF. 64ms is ~4 frames at
 *  60Hz, and far below the cadence of a real finger scroll. */
export const NUDGE_EVENT_WINDOW_MS = 64;

/** Whether a `scroll` event arriving right now is likely the compositor-recovery
 *  nudge below rather than the user.
 *
 *  The nudge writes 1px and puts it back a frame later, so both writes fire a
 *  real `scroll` event on the container. A consumer that turns scroll deltas
 *  into visible motion has to skip them, or the element it drives jitters once
 *  per nudge. On a streaming thread that reads as a permanent shake while the
 *  user is doing nothing. `useHideOnScroll` is the consumer this exists for.
 *
 *  Deliberately a time window and not a counter: a counter would have to be
 *  decremented a frame after the restore through every supersede and teardown
 *  path, and one leaked increment would pin the header forever. A stale window
 *  costs at most one skipped scroll event. */
export function isRepaintNudging(now: number = performance.now()): boolean {
  return now - lastNudgeAt < NUDGE_EVENT_WINDOW_MS;
}

/** Every `scrollTop` write the nudge makes goes through here, so `lastNudgeAt`
 *  cannot fall out of sync with the writes it is meant to describe. */
function writeNudgedScrollTop(el: HTMLElement, top: number) {
  lastNudgeAt = performance.now();
  el.scrollTop = top;
}

/** The transform for the nudged frame: this toggle's scroll compensation, the
 *  caller's own baseline, and the `translateZ` that is the only lever a
 *  non-scrollable element has.
 *
 *  A scroll of `d` paints the content `d` higher, so translating the container
 *  by `d` puts it back where the reader is looking. The layer still gets the
 *  offset change that re-tiles it, and nothing appears to move. Without the
 *  compensation every toggle is a painted 1px flick, and the callers fire up to
 *  five a second while a reply streams. It cancels only what the scroll moved,
 *  so a scrollport-pinned child undoes it again: see `publishPinnedShift`.
 *
 *  The compensation LEADS, because transform functions apply left to right in
 *  each other's coordinate systems. Behind a `scale(2)` it would move two
 *  viewport pixels rather than one.
 *
 *  A toggle that did not nudge compensates nothing, so a non-scrollable element
 *  gets the bare `translateZ` it has always had. Why the one-frame shift of the
 *  container's own box is invisible:
 *  docs/plans/2026-08-15-the-repaint-nudge-stops-shaking-the-transcript.md */
function nudgedTransform(prev: string, entry: InFlightToggle): string {
  const delta = nudgeDelta(entry);
  const compensation = delta === 0 ? '' : `translateY(${delta}px) `;
  return `${compensation}${prev ? `${prev} ` : ''}translateZ(0.1px)`;
}

/** How far this toggle moved the scroll, and therefore how far its compensation
 *  moves the container. Zero where the nudge was skipped. One reading, shared by
 *  the transform and the pinned child's counter, so the two cannot disagree. */
function nudgeDelta(entry: InFlightToggle): number {
  return entry.nudgedTop === undefined ? 0 : entry.nudgedTop - entry.restoreTop!;
}

/** Hand a scrollport-pinned child the shift it has to undo.
 *
 *  The compensation above cancels the scroll for everything the scroll MOVED. A
 *  `position: sticky` child is not that: it paints at its own `top` whatever
 *  `scrollTop` says, so the scroll leg passes it by and the transform leg is an
 *  uncancelled displacement. It reads this property and subtracts it in its own
 *  `transform` (the mobile thread title bar, styles/mobile.css). Left alone it
 *  flicks a pixel per toggle, which is five times on a thread open and a
 *  continuous shimmer while a reply streams.
 *
 *  Written on the CHILD, never on the scroller or the root. A custom property
 *  inherits, so a write one level up would invalidate style for the whole
 *  transcript on every nudge. `useHideOnScroll`'s `bindOffsetConsumers` carries
 *  the same reasoning, and asks a third consumer to opt in here rather than
 *  reinstate that recalc. */
function publishPinnedShift(el: HTMLElement, entry: InFlightToggle) {
  const delta = nudgeDelta(entry);
  if (delta === 0) return;
  // The scroller's OWN children, scanned directly. A `:scope > [attr]` query
  // has no fast path in either engine, since the rightmost compound is an
  // attribute selector. It would walk the whole transcript on a frame that then
  // forces layout. Every marked child, not the first: the marker invites a
  // second one, and serving only one would displace the rest silently.
  entry.pinned = Array.from(el.children).filter(
    child => child.hasAttribute(SCROLLER_PINNED_ATTR),
  ) as HTMLElement[];
  for (const pinned of entry.pinned) pinned.style.setProperty(PINNED_SHIFT_PROP, `${delta}px`);
}

/** Retire the counter. Called wherever the container's transform goes back to
 *  its baseline, which is the displacement the counter exists to undo. That is
 *  deliberately NOT tied to the scroll restore: `restoreNudge` yields to a
 *  concurrent writer, and the transform returns to the baseline regardless. */
function clearPinnedShift(entry: InFlightToggle) {
  for (const pinned of entry.pinned) pinned.style.removeProperty(PINNED_SHIFT_PROP);
  entry.pinned = [];
}

/** Undo a toggle's OWN scroll nudge: restore the value it nudged FROM, only if
 *  `scrollTop` is still the value it nudged TO. If anything else moved it
 *  meanwhile (a saved-position restore on open, a chevron tap, a real user
 *  scroll), yield and leave it. Never clobber a concurrent writer. */
function restoreNudge(el: HTMLElement, entry: InFlightToggle) {
  if (entry.nudgedTop !== undefined && el.scrollTop === entry.nudgedTop) {
    writeNudgedScrollTop(el, entry.restoreTop!);
  }
}

export function forceIOSRepaint(el: HTMLElement | null | undefined): (() => void) | undefined {
  if (!el?.isConnected || !isIOS()) return;

  // A burst is common: a single iOS PWA resume fires visibilitychange +
  // pageshow + focus in one tick, each triggering a repaint. SUPERSEDE any
  // in-flight toggle instead of skipping the new call. iOS suspends a
  // backgrounded PWA and can DROP its queued rAF callbacks, leaving a
  // "pending" flag set forever. A skip keyed on that flag would lock the
  // element out of every later repaint.
  const existing = inFlight.get(el);
  // Reuse the transform baseline across a burst so overlapping resume events
  // cannot capture an intermediate nudged value and accumulate
  // `translateZ(0.1px) translateZ(0.1px)`. The scroll baseline is NOT carried
  // over: each toggle re-reads the live scrollTop in raf1.
  const prev = existing ? existing.prev : el.style.transform;
  // Decide ONCE at burst start whether to nudge `scrollTop`, and reuse that
  // decision on every superseding call so rAF1 and rAF2 always agree. Three
  // gates. The element must be scrollable. No notification deep-link scroll
  // claim may be in flight: this fires on the same iOS resume signals a
  // notification-tap foreground triggers, and a nudge would fight
  // scrollToEventAndPulse's landing. And no user drag or momentum scroll may
  // be in flight, since iOS cancels a fling on ANY scrollTop write. The
  // transform round-trip and forced layout read below repaint regardless.
  const nudgeScroll = existing
    ? existing.nudgeScroll
    : el.scrollHeight > el.clientHeight && !hasPendingEventScroll() && !isUserScrolling();

  if (existing) {
    cancelAnimationFrame(existing.raf1);
    if (existing.raf2 !== undefined) cancelAnimationFrame(existing.raf2);
    // Undo any partial nudge so the fresh toggle is a real round-trip from the
    // baseline: re-writing the same value can be coalesced away. Restoring the
    // prior toggle's nudged-FROM value keeps the next raf1 reading a clean live
    // position, with no 1px-per-burst drift.
    if (el.style.transform !== prev) el.style.transform = prev;
    clearPinnedShift(existing);
    restoreNudge(el, existing);
  }

  const entry: InFlightToggle = { raf1: 0, raf2: undefined, prev, nudgeScroll, restoreTop: undefined, nudgedTop: undefined, pinned: [] };
  inFlight.set(el, entry);
  // Only clear the slot if it is still ours: a later superseding call may have
  // replaced the entry, and a dropped-then-resumed stale frame must not evict
  // it.
  const done = () => { if (inFlight.get(el) === entry) inFlight.delete(el); };
  entry.raf1 = requestAnimationFrame(() => {
    if (!el.isConnected) { done(); return; }
    // A real scrollTop change forces WKWebView to re-commit the frozen layer
    // tree, the way a manual scroll recovers the blank.
    //
    // Re-baseline from the LIVE scrollTop HERE, not at call time: the reader
    // can move between the call and this frame, and a call-time baseline would
    // write a position they have already left. A true 1px stays inside the 2px
    // chevron slack (scrollState.ts), so it moves nobody and trips nothing.
    //
    // Re-check the live scroll state at the write point too. The callers are
    // timer-driven and this write is deferred a frame, so a fling beginning
    // after an idle burst decision would otherwise be cancelled. Skipping here
    // leaves `nudgedTop` unset, so the rAF2 restore is a no-op.
    //
    // Decide the nudge BEFORE the transform, which carries its compensation.
    if (nudgeScroll && !isUserScrolling()) {
      const live = el.scrollTop;
      // Direction-safe so it never clamps to a no-op at either extreme.
      entry.restoreTop = live;
      entry.nudgedTop = live > 0 ? live - 1 : live + 1;
    }
    el.style.transform = nudgedTransform(prev, entry);
    publishPinnedShift(el, entry);
    if (entry.nudgedTop !== undefined) writeNudgedScrollTop(el, entry.nudgedTop);
    // Force a synchronous layout flush so the nudged state paints this frame
    // rather than being coalesced with the restore on the next one. Both writes
    // above are in it, so the compensation lands on the frame it cancels.
    void el.offsetHeight;
    entry.raf2 = requestAnimationFrame(() => {
      if (el.isConnected) {
        el.style.transform = prev;
        clearPinnedShift(entry);
        restoreNudge(el, entry);
      }
      done();
    });
  });
  return () => {
    cancelAnimationFrame(entry.raf1);
    if (entry.raf2 !== undefined) cancelAnimationFrame(entry.raf2);
    // Restore both baselines so an explicit cleanup mid-toggle never leaves a
    // stale nudge behind (which the next call would otherwise read as baseline).
    if (el.style.transform !== prev) el.style.transform = prev;
    clearPinnedShift(entry);
    restoreNudge(el, entry);
    done();
  };
}

/** Delays (ms) for the thread-OPEN repaint burst: an immediate toggle plus
 *  setTimeout-spaced retries. setTimeout rather than chained rAFs is
 *  load-bearing, because it survives the frame coalescing that can swallow a
 *  one-shot toggle on a cold open. The span also covers a layer that blanks a
 *  beat AFTER the initial paint.
 *
 *  The tail reaches 1000ms because a long-lived iOS PWA session degrades:
 *  WKWebView blanks an already-loaded thread's layer noticeably later than a
 *  cold open does. Each attempt is a full supersede-safe toggle, so they never
 *  accumulate and cost nothing on a healthy layer. Exported for the invariant
 *  test. */
export const OPEN_REPAINT_BURST_DELAYS_MS = [0, 100, 300, 600, 1000];

/** Drop-resilient repaint for the thread-OPEN path. The streaming path re-fires
 *  on a trailing throttle and the resume path on visibilitychange / pageshow /
 *  focus, but the open path fires ONCE. On a cold open iOS can drop the
 *  toggle's two queued rAF callbacks, or run them before the scroll layer is
 *  blanked. There is no retry to recover it.
 *
 *  Fires several attempts spaced over a few hundred ms, so one dropped frame
 *  cannot swallow the whole recovery. Each is a full supersede-safe toggle, so
 *  they never accumulate, and each only touches `transform` so scroll math is
 *  untouched. No-op off iOS and for a detached node. Returns a cleanup that
 *  cancels the immediate toggle's frames AND every pending retry. */
export function forceIOSRepaintBurst(el: HTMLElement | null | undefined): (() => void) | undefined {
  if (!el?.isConnected || !isIOS()) return;
  let immediateCleanup: (() => void) | undefined;
  const timers: Array<ReturnType<typeof setTimeout>> = [];
  for (const delay of OPEN_REPAINT_BURST_DELAYS_MS) {
    if (delay === 0) {
      immediateCleanup = forceIOSRepaint(el);
    } else {
      timers.push(setTimeout(() => forceIOSRepaint(el), delay));
    }
  }
  return () => {
    immediateCleanup?.();
    for (const t of timers) clearTimeout(t);
    timers.length = 0;
  };
}

/** Leading + trailing repaint throttle. Call `request(now, fire)` on every
 *  repaint request; `fire` runs immediately on the LEADING edge, at most once
 *  per `intervalMs`. A request arriving inside the window is not dropped: it
 *  re-arms a single TRAILING `fire` for the end of the window, so the last
 *  request before activity stops always paints. `cancel()` clears a pending
 *  trailing fire. The caller supplies `now`; only the trailing edge
 *  self-schedules.
 *
 *  A streaming thread appends content many times a second, and each append can
 *  make iOS WKWebView blank the scroll container's layer. Repainting per token
 *  would thrash the compositor.
 *
 *  The trailing edge is load-bearing. A streamed mutation can blank the layer a
 *  beat AFTER a leading repaint fired, and a leading-only gate throttles that
 *  re-blanking request away. If the stream then pauses, nothing arrives to
 *  recover it and the content stays black until a manual scroll. */
export interface RepaintThrottle {
  request(now: number, fire: () => void): void;
  cancel(): void;
}

export function createRepaintThrottle(intervalMs: number): RepaintThrottle {
  let last = -Infinity;
  let trailingTimer: ReturnType<typeof setTimeout> | null = null;
  const clearTrailing = () => {
    if (trailingTimer !== null) {
      clearTimeout(trailingTimer);
      trailingTimer = null;
    }
  };
  return {
    request(now: number, fire: () => void) {
      // Any fresh request supersedes a pending trailing fire: either it fires
      // now on the leading edge, or it is re-armed for the new window end.
      clearTrailing();
      const elapsed = now - last;
      if (elapsed >= intervalMs) {
        last = now;
        fire();
        return;
      }
      const wait = intervalMs - elapsed;
      trailingTimer = setTimeout(() => {
        trailingTimer = null;
        last = now + wait; // projected fire time, in the same timebase as `now`
        fire();
      }, wait);
    },
    cancel: clearTrailing,
  };
}
