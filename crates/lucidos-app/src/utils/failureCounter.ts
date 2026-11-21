/**
 * Tracks consecutive failures of a best-effort async operation; invokes
 * `onThreshold` exactly once when the count first crosses `threshold`, then
 * stays quiet until the next `recordSuccess()` resets it.
 *
 * Use for paths that run on a schedule (polling, periodic refresh) where:
 *   - a single failure shouldn't bother the user (transient hiccup)
 *   - N failures in a row mean something is genuinely wrong and the user
 *     should know
 *   - the surface that knows about the failure (caller) is also the one
 *     responsible for telling the user (no separate error pipeline)
 *
 * See `.claude/rules/frontend.md` § "Carve-out: best-effort telemetry"
 * for when to reach for this vs. a per-failure toast vs. silence.
 */
export function createFailureCounter(threshold: number, onThreshold: () => void) {
  if (threshold <= 0) {
    throw new Error(`createFailureCounter: threshold must be > 0, got ${threshold}`);
  }
  let count = 0;
  let notified = false;
  return {
    recordFailure(): void {
      count++;
      if (count >= threshold && !notified) {
        notified = true;
        onThreshold();
      }
    },
    recordSuccess(): void {
      count = 0;
      notified = false;
    },
  };
}
