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
 *  background read paths (`refreshChangesState`, `loadUnreadNotifications`,
 *  `refreshThreadEvents`) attach no manual `AbortController`, so an AbortError
 *  there is never user-initiated: it's the browser cancelling an in-flight
 *  request when an iOS PWA freezes/backgrounds mid-fetch or the connection
 *  resets on a radio handoff. That carries no reachability signal and the next
 *  resume re-syncs, so those callers suppress it instead of surfacing
 *  "request cancelled" or counting it as an outage. A real client-side timeout
 *  fires `TimeoutError` (distinct, still surfaces); a genuine unreachable engine
 *  fires `TimeoutError` or a transport `TypeError` (see `isTransportError`). */
export function isAbortError(err: unknown): boolean {
  return err instanceof DOMException && err.name === 'AbortError';
}
