import { isIOS } from './platform';
import { hasPendingEventScroll } from '../components/chat/scrollState';
import { isUserScrolling } from './scrollActivity';

/** Force iOS Safari / WKWebView to repaint a compositor layer whose backing
 *  texture it has blanked — content is present in the DOM (events loaded,
 *  exchanges rendered, real laid-out height) but renders black / invisible. This
 *  is the documented WKCompositingView-removal class of blank: WebKit stops
 *  committing the layer tree (`_didCommitLayerTree`), so the layer freezes on a
 *  stale-or-empty texture until something forces a fresh commit — a scroll,
 *  rotation, or layout change. (Distinct from WebContent-process death, which
 *  fires `webViewWebContentProcessDidTerminate`; the iOS PWA is Safari's own
 *  WKWebView, so a frontend repaint is the ONLY recovery lever on that surface.)
 *
 *  Three layered, additive nudges across two animation frames — applied in frame
 *  one, restored in frame two — so the intermediate state is genuinely painted
 *  and can't be coalesced into a no-op:
 *    1. A sub-pixel `translateZ` transform round-trip (the original primitive;
 *       cheap, and the only lever for a non-scrollable element).
 *    2. A forced synchronous layout read (`void el.offsetHeight`) after the
 *       nudge — the documented most-reliable JS repaint trigger; flushes the
 *       pending layout so the nudged frame paints instead of being swallowed.
 *    3. A real ±1px `scrollTop` nudge on a scrollable element — re-commits the
 *       layer tree exactly as a manual finger-scroll does (the user-confirmed
 *       recovery). Two refinements:
 *       - The nudge is taken ±1 from the LIVE scrollTop at NUDGE time (frame one),
 *         not from a call-time baseline: during streaming the bottom moves between
 *         the call and the frame, and a stale baseline would nudge far above the
 *         grown bottom and leave the reader parked there. A true ±1 stays inside
 *         the 2px chevron slack (scrollState.ts), so it never trips the chevron.
 *         The restore yields: it puts `scrollTop` back to the value the nudge
 *         came FROM only if our nudge is still the current value, so a concurrent
 *         scroll write (`useScrollMemory`'s saved-position restore on thread
 *         open, a chevron tap) is never clobbered.
 *       - The scroll-nudge is skipped entirely while a notification deep-link
 *         scroll claim is in flight (`hasPendingEventScroll()`): the same iOS
 *         resume signals (visibilitychange/pageshow/focus) that fire this repaint
 *         also fire when a notification-tap foregrounds the PWA, exactly as
 *         `scrollToEventAndPulse` is smooth-scrolling to the target event; a nudge
 *         would fight that landing and drop the user mid-thread.
 *       - The scroll-nudge is ALSO skipped while a user touch-drag / its momentum
 *         tail is in flight (`isUserScrolling()`): iOS cancels an in-flight
 *         momentum scroll the instant scrollTop is written, so a nudge ticking on
 *         a timer (streaming throttle, open burst, settle probe, resume) mid-fling
 *         stops the scroll dead — the "scrolling randomly stops instead of going
 *         further when you let go" bug. A live scroll already keeps the compositor
 *         committing, so the nudge is redundant then anyway. See scrollActivity.ts.
 *         The decision is frozen at burst start (and reused on supersede) so rAF1
 *         and rAF2 always agree. The transform round-trip + forced layout read (1 & 2)
 *         still force the compositor re-commit without touching `scrollTop`.
 *  Plain `translateZ` alone is widely reported as unreliable (it stops flicker but
 *  the repaint can still not land), which matches the field reports this fixes.
 *
 *  No-op off iOS and for detached nodes. Returns a cleanup that cancels the
 *  pending frames AND restores both baselines so a caller can hand it straight
 *  back from a `useEffect`; fire-and-forget callers may ignore it. */

/** Per-element in-flight toggle. Tracks the scheduled frames and the transform
 *  baseline (captured once on the first call of a burst) plus this toggle's own
 *  scroll nudge — captured at NUDGE time (raf1) from the LIVE scrollTop, NOT at
 *  call time — so a superseding call can undo a partial nudge cleanly. */
interface InFlightToggle {
  raf1: number;
  raf2: number | undefined;
  prev: string;
  /** "Should we nudge `scrollTop`?" decided ONCE at burst start and reused on
   *  every superseding call, so rAF1 (nudge) and rAF2 (restore) always agree even
   *  though `hasPendingEventScroll()` can flip mid-burst — never
   *  nudge-then-skip-restore or skip-then-restore. False when the element isn't
   *  scrollable OR a deep-link claim is in flight; gates the scroll write so those
   *  cases never touch `scrollTop`. */
  nudgeScroll: boolean;
  /** The LIVE scrollTop this toggle's raf1 nudged FROM, and the value it nudged
   *  TO — captured IN raf1 (not at call time) so streaming growth between the call
   *  and the nudge can't make the nudge stale. `undefined` until raf1 has actually
   *  nudged (nudgeScroll was true), so `restoreNudge` is a no-op until then. */
  restoreTop: number | undefined;
  nudgedTop: number | undefined;
}
const inFlight = new WeakMap<object, InFlightToggle>();

/** `performance.now()` of the most recent `scrollTop` write made by the nudge. */
let lastNudgeAt = -Infinity;

/** How long after a nudge write its `scroll` event is still expected to arrive.
 *  A `scrollTop` write does not dispatch `scroll` synchronously: the event is
 *  queued and fired in the next frame's "run the scroll steps", BEFORE that
 *  frame's rAF callbacks. So the window has to outlive the write by a frame,
 *  which rules out clearing a flag in the restoring rAF. 64ms is ~4 frames at
 *  60Hz, generous enough for a loaded main thread and far below the cadence of
 *  a real finger scroll. */
export const NUDGE_EVENT_WINDOW_MS = 64;

/** Whether a `scroll` event arriving right now is likely the compositor-recovery
 *  nudge below rather than the user.
 *
 *  The nudge writes ±1px and puts it back a frame later, so both writes fire a
 *  real `scroll` event on the container. A consumer that turns scroll deltas into
 *  visible motion has to skip them, or the element it drives jitters once per
 *  nudge. On a streaming thread the nudge runs on a ~200ms throttle
 *  (`ThreadView`'s `streamRepaintGate`), so that reads as a permanent shake while
 *  the user is doing nothing at all. `useHideOnScroll` is the consumer this
 *  exists for; `ContentPane` documents the same collision as the reason it fires
 *  the repaint on resume only.
 *
 *  Deliberately a time window and not a counter: a counter would have to be
 *  decremented a frame after the restore (see NUDGE_EVENT_WINDOW_MS) through
 *  every supersede and teardown path, and a single leaked increment would pin the
 *  header forever. A stale window costs at most one skipped scroll event. */
export function isRepaintNudging(now: number = performance.now()): boolean {
  return now - lastNudgeAt < NUDGE_EVENT_WINDOW_MS;
}

/** Every `scrollTop` write the nudge makes goes through here, so `lastNudgeAt`
 *  cannot fall out of sync with the writes it is meant to describe. */
function writeNudgedScrollTop(el: HTMLElement, top: number) {
  lastNudgeAt = performance.now();
  el.scrollTop = top;
}

/** Undo a toggle's OWN scroll nudge — restore to the value it nudged FROM, iff
 *  `scrollTop` is still the value it nudged TO. If anything else moved it
 *  meanwhile (useScrollMemory's saved-position restore on open, a chevron tap,
 *  a real user scroll), yield and leave it: never clobber a concurrent writer.
 *  No-op until raf1 has actually nudged. */
function restoreNudge(el: HTMLElement, entry: InFlightToggle) {
  // `nudgedTop` is set only when raf1 actually nudged (nudgeScroll was true), so
  // this is implicitly a no-op for the non-scrollable / deep-link-claim cases.
  if (entry.nudgedTop !== undefined && el.scrollTop === entry.nudgedTop) {
    writeNudgedScrollTop(el, entry.restoreTop!);
  }
}

export function forceIOSRepaint(el: HTMLElement | null | undefined): (() => void) | undefined {
  if (!el?.isConnected || !isIOS()) return;

  // A burst is common — a single iOS PWA resume fires visibilitychange +
  // pageshow + focus in one tick, each triggering a repaint. SUPERSEDE any
  // in-flight toggle instead of skipping the new call.
  //
  // Skipping (the previous behavior) could lock an element out of all future
  // repaints: iOS suspends a backgrounded PWA and can DROP its queued rAF
  // callbacks rather than deferring them. If the page froze between scheduling
  // the toggle and its second frame, the "pending" flag never cleared, so every
  // later repaint — including the resume / open-thread repaint meant to un-blank
  // the layer — no-op'd, and the thread content stayed black until the element
  // was recreated. Cancelling the stale frames and rescheduling means a resume
  // repaint always lands.
  const existing = inFlight.get(el);
  // Reuse the transform baseline across a burst so overlapping resume events
  // can't capture an intermediate nudged value and accumulate
  // `translateZ(0.1px) translateZ(0.1px) …`. The scroll baseline is NOT carried
  // over — each toggle re-reads the live scrollTop in raf1 (see below).
  const prev = existing ? existing.prev : el.style.transform;
  // Decide ONCE at burst start whether to nudge `scrollTop`, and reuse that
  // decision on every superseding call so rAF1 and rAF2 always agree (see
  // InFlightToggle.nudgeScroll). Three gates: the element must be scrollable; no
  // notification deep-link scroll claim may be in flight — `forceIOSRepaint` fires
  // on the same iOS resume signals (visibilitychange/pageshow/focus) a
  // notification-tap foreground triggers, exactly as scrollToEventAndPulse is
  // smooth-scrolling to the target event, and a ±1px nudge would fight that
  // landing; AND no user touch-drag / momentum scroll may be in flight
  // (isUserScrolling) — iOS cancels an in-flight momentum scroll on ANY scrollTop
  // write, so a nudge ticking on a timer mid-fling stops it dead (the "scrolling
  // randomly stops when you let go" bug). The transform round-trip + forced layout
  // read below still repaint regardless.
  const nudgeScroll = existing
    ? existing.nudgeScroll
    : el.scrollHeight > el.clientHeight && !hasPendingEventScroll() && !isUserScrolling();

  if (existing) {
    cancelAnimationFrame(existing.raf1);
    if (existing.raf2 !== undefined) cancelAnimationFrame(existing.raf2);
    // Undo any partial nudge so the fresh toggle is a real round-trip from the
    // baseline — re-writing the same value can be coalesced away without forcing
    // the repaint we're after. Restoring to the prior toggle's nudged-FROM value
    // means the next raf1 re-reads a clean live position (no 1px-per-burst drift).
    if (el.style.transform !== prev) el.style.transform = prev;
    restoreNudge(el, existing);
  }

  const entry: InFlightToggle = { raf1: 0, raf2: undefined, prev, nudgeScroll, restoreTop: undefined, nudgedTop: undefined };
  inFlight.set(el, entry);
  // Only clear the slot if it's still ours — a later superseding call may have
  // replaced the entry, and a dropped-then-resumed stale frame must not evict it.
  const done = () => { if (inFlight.get(el) === entry) inFlight.delete(el); };
  entry.raf1 = requestAnimationFrame(() => {
    if (!el.isConnected) { done(); return; }
    el.style.transform = prev ? `${prev} translateZ(0.1px)` : 'translateZ(0.1px)';
    // ±1px scroll nudge — a real scrollTop change forces WKWebView to re-commit
    // the frozen layer tree, the way a manual scroll recovers the blank. Skipped
    // when a deep-link claim is in flight (folded into `nudgeScroll`) so it can't
    // fight scrollToEventAndPulse's landing on iOS resume.
    //
    // Re-baseline from the LIVE scrollTop HERE (nudge time), not at call time:
    // the reader can move between the call and this frame (a chevron tap, a
    // scroll), and a call-time baseline would then write a position they have
    // already left, moving them backwards by however far they got. Reading the
    // live position keeps the nudge a true ±1, inside the 2px chevron slack
    // (scrollState.ts), so it moves nobody and trips nothing.
    // Re-check the LIVE scroll state at the write point, not just at burst start:
    // the callers are timer/data-driven and this write is deferred a frame, so a
    // fling that begins in the ~16ms between an idle call and this frame would
    // otherwise be cancelled by the nudge (the burst decision was frozen while
    // idle). Skipping here leaves `nudgedTop` unset, so the rAF2 restore is a
    // no-op — rAF1/rAF2 stay consistent (see restoreNudge). The transform +
    // forced layout read below still repaint regardless.
    if (nudgeScroll && !isUserScrolling()) {
      const live = el.scrollTop;
      // Direction-safe so it never clamps to a no-op at either extreme.
      const nudged = live > 0 ? live - 1 : live + 1;
      entry.restoreTop = live;
      entry.nudgedTop = nudged;
      writeNudgedScrollTop(el, nudged);
    }
    // Force a synchronous layout flush so the nudged state actually paints this
    // frame (the documented reliable repaint trigger) rather than being coalesced
    // with the restore on the next frame.
    void el.offsetHeight;
    entry.raf2 = requestAnimationFrame(() => {
      if (el.isConnected) {
        el.style.transform = prev;
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
    restoreNudge(el, entry);
    done();
  };
}

/** Delays (ms) for the thread-OPEN repaint burst: an immediate toggle plus
 *  setTimeout-spaced retries. setTimeout (not chained rAFs) is the load-bearing
 *  choice — it survives the frame coalescing/dropping that can swallow a one-shot
 *  toggle on a cold open. The span also covers a layer that blanks a beat AFTER
 *  the initial mount/paint, which an immediate-only toggle has already restored
 *  past before the blank lands.
 *
 *  The tail reaches 1000ms (was 300ms): under PROLONGED use — many thread switches
 *  in one long-lived iOS PWA session — WKWebView degrades and blanks an
 *  already-loaded thread's layer noticeably later than a cold open does, past the
 *  old 300ms window, so the open burst's last attempt fired before the blank and
 *  the thread stayed black until a manual scroll. Reported as "after a while,
 *  opening a thread shows an empty body" (the body is in the DOM — no skeleton, no
 *  empty-state — it just isn't painted). The extra spaced attempts are each a
 *  full supersede-safe toggle, so they never accumulate and cost nothing on a
 *  healthy layer (the toggle is a sub-pixel transform round-trip). Exported for
 *  the invariant test. */
export const OPEN_REPAINT_BURST_DELAYS_MS = [0, 100, 300, 600, 1000];

/** Drop-resilient repaint for the thread-OPEN path. A single `forceIOSRepaint`
 *  suffices for the streaming path (a trailing throttle re-fires) and the resume
 *  path (re-fires on visibilitychange / pageshow / focus), but the open path
 *  fires ONCE — and on a cold open iOS can drop/coalesce the toggle's two queued
 *  rAF callbacks, or run them before the freshly-mounted scroll layer is actually
 *  blanked. Either way the one-shot recovery is swallowed and the thread renders
 *  black (DOM present, scroll chevron visible) until a manual scroll; unlike the
 *  streaming/resume paths there is no retry to recover it.
 *
 *  Fires several `forceIOSRepaint` attempts spaced over a few hundred ms so a
 *  single dropped frame can't swallow the whole recovery, and a later attempt
 *  catches a layer that blanked after the earlier ones restored. Each attempt is
 *  a full supersede-safe toggle (see `forceIOSRepaint`), so they never
 *  accumulate, and each only touches `transform` so scroll math is untouched.
 *  No-op off iOS / for a detached node. Returns a cleanup that cancels the
 *  immediate toggle's frames AND every pending retry — hand it straight back
 *  from a `useEffect`. */
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
 *  repaint request; `fire` runs immediately on the LEADING edge (at most once
 *  per `intervalMs`). A request that arrives inside the throttle window is NOT
 *  dropped — it (re-)arms a single TRAILING `fire` for the end of the window, so
 *  the LAST request before activity stops always paints. `cancel()` clears a
 *  pending trailing fire (call on teardown). The caller supplies `now` (e.g.
 *  `performance.now()`); only the trailing edge self-schedules (via setTimeout).
 *
 *  Why: a streaming thread appends content many times a second, and each append
 *  can make iOS WKWebView blank the scroll container's compositor layer (content
 *  in the DOM, renders black). Forcing a repaint per token would thrash the
 *  compositor; one repaint every couple hundred ms keeps the layer healthy.
 *
 *  The trailing edge is load-bearing. A streamed mutation (or the large DOM
 *  shrink from a turn control on the running thread) can blank the
 *  layer a beat AFTER a leading repaint already fired. With a leading-only gate
 *  that re-blanking request is throttled away, and if the stream then pauses (a
 *  Claude Code tool call running for many seconds) no further request arrives to
 *  recover it — the thread content stays black until a manual scroll. The
 *  trailing repaint clears it within one window. (This is the "clicking Less on
 *  the last running result blanks the pane" report.) */
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
      // Any fresh request supersedes a pending trailing fire — either we fire now
      // (leading) or we re-arm it for the new window end below.
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
