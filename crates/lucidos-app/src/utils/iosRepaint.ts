import { isIOS } from './platform';

/** Force iOS Safari / WKWebView to repaint a compositor layer whose backing
 *  texture it has blanked — content is present in the DOM but renders black /
 *  invisible. iOS recycles layer textures inside scroll containers (scroll-snap
 *  / momentum-scroll parents); a large DOM mutation or an `overflow` freeze can
 *  leave the layer showing a stale-or-empty texture with no repaint scheduled.
 *
 *  Toggling a sub-pixel `translateZ` across two animation frames invalidates the
 *  cached texture and forces a fresh paint — the same effect a real scroll
 *  gesture has, applied proactively. No-op off iOS and for detached nodes.
 *
 *  Returns a cleanup that cancels the pending frames so a caller can hand it
 *  straight back from a `useEffect`; fire-and-forget callers may ignore it. */

/** Per-element in-flight toggle. Tracks the scheduled frames and the TRUE
 *  baseline transform (captured once, on the first call of a burst) so a
 *  superseding call restores cleanly instead of capturing an intermediate
 *  nudge as its baseline. */
interface InFlightToggle {
  raf1: number;
  raf2: number | undefined;
  prev: string;
}
const inFlight = new WeakMap<object, InFlightToggle>();

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
  // Capture the baseline ONCE and reuse it for every superseding call in the
  // burst, so overlapping resume events can't capture an intermediate nudged
  // value and accumulate `translateZ(0.1px) translateZ(0.1px) …`.
  const prev = existing ? existing.prev : el.style.transform;
  if (existing) {
    cancelAnimationFrame(existing.raf1);
    if (existing.raf2 !== undefined) cancelAnimationFrame(existing.raf2);
    // Undo any partial nudge so the fresh toggle is a real round-trip from the
    // baseline — re-writing the same transform value can be coalesced away by
    // the engine without forcing the repaint we're after.
    if (el.style.transform !== prev) el.style.transform = prev;
  }

  const entry: InFlightToggle = { raf1: 0, raf2: undefined, prev };
  inFlight.set(el, entry);
  // Only clear the slot if it's still ours — a later superseding call may have
  // replaced the entry, and a dropped-then-resumed stale frame must not evict it.
  const done = () => { if (inFlight.get(el) === entry) inFlight.delete(el); };
  entry.raf1 = requestAnimationFrame(() => {
    if (!el.isConnected) { done(); return; }
    el.style.transform = prev ? `${prev} translateZ(0.1px)` : 'translateZ(0.1px)';
    entry.raf2 = requestAnimationFrame(() => {
      if (el.isConnected) el.style.transform = prev;
      done();
    });
  });
  return () => {
    cancelAnimationFrame(entry.raf1);
    if (entry.raf2 !== undefined) cancelAnimationFrame(entry.raf2);
    // Restore the baseline so an explicit cleanup mid-toggle never leaves a
    // stale nudge behind (which the next call would otherwise read as baseline).
    if (el.style.transform !== prev) el.style.transform = prev;
    done();
  };
}

/** Delays (ms) for the thread-OPEN repaint burst: an immediate toggle plus
 *  setTimeout-spaced retries. setTimeout (not chained rAFs) is the load-bearing
 *  choice — it survives the frame coalescing/dropping that can swallow a one-shot
 *  toggle on a cold open. The ~300ms span also covers a layer that blanks a beat
 *  AFTER the initial mount/paint, which an immediate-only toggle has already
 *  restored past before the blank lands. */
const OPEN_REPAINT_BURST_DELAYS_MS = [0, 100, 300];

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
 *  The trailing edge is load-bearing: a streamed mutation — or the large DOM
 *  shrink from a More/Less / steps toggle on the running thread — can blank the
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
