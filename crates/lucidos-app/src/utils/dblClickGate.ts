const DEFAULT_MAX_MS = 400;

/**
 * Gates double-click detection to a strict time window, overriding
 * the OS-level double-click interval which can be very generous.
 *
 * Call `record()` on each click/pointerdown, then `allow()` in the
 * dblclick handler to check if the gap was within threshold.
 */
export function createDblClickGate(maxMs = DEFAULT_MAX_MS) {
  let prev = 0;
  let current = 0;
  return {
    record() {
      prev = current;
      current = Date.now();
    },
    allow(): boolean {
      return current > prev && current - prev <= maxMs;
    },
  };
}
