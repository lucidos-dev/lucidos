/**
 * Admits the FIRST call in a burst and refuses the rest until the window
 * elapses (a leading-edge throttle, expressed as a predicate rather than a
 * wrapper so the caller keeps its own control flow).
 *
 * Built for the iOS PWA wake burst: WebKit fires `visibilitychange`, `focus`
 * and `pageshow` together on a single resume, so a handler bound to all three
 * runs its whole reconciliation fan-out three times per wake. Leading edge
 * specifically, because the work must be deduplicated, never delayed.
 *
 * Deliberately NOT a trailing-edge debounce and NOT a promise-based in-flight
 * guard: the burst members arrive within the same tick or a few hundred
 * milliseconds of each other, and the work behind them is a set of independent
 * fire-and-forget calls with no single settle point to await.
 */
export function createLeadingEdgeGate(windowMs: number) {
  if (windowMs <= 0) {
    throw new Error(`createLeadingEdgeGate: windowMs must be > 0, got ${windowMs}`);
  }
  let lastAdmitted: number | null = null;
  return {
    /** True for the first call in a burst, false for the rest of the window. */
    allow(): boolean {
      const now = Date.now();
      // Strict `<` on the elapsed check, so a window of N ms admits again
      // exactly N ms later rather than needing N+1. The boundary is otherwise
      // unreachable under a mocked clock that advances in exact steps.
      if (lastAdmitted !== null && now - lastAdmitted < windowMs) return false;
      lastAdmitted = now;
      return true;
    },
  };
}
