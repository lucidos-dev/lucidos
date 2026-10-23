/** Format error detail for toast messages. Handles AbortError, Error, and unknown shapes. */
export function errorDetail(err: unknown): string {
  if (err instanceof DOMException) {
    // Distinguish "we gave up" (timeout) from "you stopped it" (manual abort).
    if (err.name === 'TimeoutError') return 'request timed out';
    if (err.name === 'AbortError') return 'request cancelled';
  }
  if (err instanceof Error) return err.message;
  return String(err);
}

/** True when a fetch rejected because the request was *cancelled* — an
 *  `AbortError` DOMException — rather than because it failed or timed out. The
 *  two background reads still gating here (`refreshChangesState`,
 *  `loadUnreadNotifications`) attach no manual `AbortController`, so an
 *  AbortError there is never user-initiated: it's the browser cancelling an
 *  in-flight request when an iOS PWA freezes/backgrounds mid-fetch or the
 *  connection resets on a radio handoff. That carries no reachability signal and
 *  the next resume re-syncs, so those callers suppress it instead of surfacing
 *  "request cancelled" or counting it as an outage. They treat a transport
 *  `TypeError` (Safari "Load failed" on a stale connection, see
 *  `isTransportError`) identically: the same iOS-PWA-wake / radio-handoff /
 *  Tailscale-reconnect noise, healed by the retry or the next resync, with the
 *  debounced connection dot owning genuine sustained outages.
 *
 *  What this predicate does NOT cover is a `TimeoutError`, and the set of
 *  callers wanting that line drawn has shrunk twice, in the same direction. It
 *  was once every background read, on the reasoning that a request which "waited
 *  the full window and got nothing" is the stronger signal. On 2026-08-04 the
 *  two per-thread event fetches in `store/actions/thread-loading.ts` moved to
 *  the wider `isTransientFetchError`, because they fan out one request per
 *  loaded thread and one dropped tunnel fires every client deadline at once. On
 *  2026-08-07 the thread-list refresh (`store/actions/thread-list-refresh.ts`)
 *  followed, and that one is NOT a fan-out: it is exactly the once-per-wake read
 *  the original argument was written for. It moved because the argument was
 *  wrong on its own terms. Over a dropped tunnel the GET hangs rather than
 *  refusing, so the deadline is what an outage looks like from the client
 *  whether one request or thirty are in flight, and the gateway log for the
 *  reported window showed the engine answering that endpoint between 12ms and 2s
 *  against a 10s deadline. Waiting the full window told us about the link, not
 *  about the engine.
 *
 *  So prefer `isTransientFetchError` for a new background READ, and reach for
 *  this narrower one only where a timeout genuinely is a verdict worth
 *  surfacing. Two WRITE paths reach the wider predicate by a different route,
 *  because a write that got no answer is owed a re-send rather than a toast:
 *  `savePreference` and the compose draft push both park the write and escalate
 *  once via `createFailureCounter`. */
export function isAbortError(err: unknown): boolean {
  return err instanceof DOMException && err.name === 'AbortError';
}
