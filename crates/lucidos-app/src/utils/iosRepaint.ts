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

/** Rate-limit gate for repeated repaint requests. Returns a predicate that
 *  yields `true` at most once per `intervalMs`, tracking its own last-allowed
 *  timestamp. The caller supplies `now` (e.g. `performance.now()`) so the gate
 *  stays pure and tests stay deterministic.
 *
 *  Why: a streaming thread appends content many times a second, and each append
 *  can make iOS WKWebView blank the scroll container's compositor layer (content
 *  in the DOM, renders black). Forcing a repaint per token would thrash the
 *  compositor and defeat the per-thread streaming gate; one repaint every couple
 *  hundred ms is enough to keep the layer from getting stuck blank. */
export function createRepaintThrottle(intervalMs: number): (now: number) => boolean {
  let last = -Infinity;
  return (now: number) => {
    if (now - last < intervalMs) return false;
    last = now;
    return true;
  };
}
