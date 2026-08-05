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
 *  background read paths (`refreshChangesState`, `loadUnreadNotifications`)
 *  attach no manual `AbortController`, so an AbortError
 *  there is never user-initiated: it's the browser cancelling an in-flight
 *  request when an iOS PWA freezes/backgrounds mid-fetch or the connection
 *  resets on a radio handoff. That carries no reachability signal and the next
 *  resume re-syncs, so those callers suppress it instead of surfacing
 *  "request cancelled" or counting it as an outage. Those same background paths
 *  treat a transport `TypeError` (Safari "Load failed" on a stale connection —
 *  see `isTransportError`) identically: it's the same iOS-PWA-wake / radio-handoff
 *  / Tailscale-reconnect noise, healed by the retry or the next resync, with the
 *  debounced connection dot owning genuine sustained outages. A real client-side
 *  timeout fires `TimeoutError` (distinct — it survives the retry and still
 *  surfaces/escalates as the stronger "waited the full window and got nothing"
 *  signal).
 *
 *  That last distinction only holds for a read issued ONCE per wake, which is
 *  why the two per-thread event fetches in `store/actions/thread-loading.ts` no
 *  longer draw the line here and gate on the wider `isTransientFetchError`
 *  instead. They are fanned out one request per loaded thread, so one dropped
 *  tunnel fires every client deadline at once and the timeout stops being
 *  evidence about the engine: it is just what an outage looks like on a fan-out.
 *
 *  Two WRITE paths reach the same conclusion by a different route, because a
 *  write that got no answer is still owed a re-send rather than a toast:
 *  `savePreference` and the compose draft push both park the write and escalate
 *  once via `createFailureCounter`. They gate on `isTransientFetchError`
 *  (`api/client/_core.ts`), which is the wider predicate: it folds a
 *  `TimeoutError` in with the aborts and transport errors, since for a write no
 *  answer of any kind means the same thing. */
export function isAbortError(err: unknown): boolean {
  return err instanceof DOMException && err.name === 'AbortError';
}
