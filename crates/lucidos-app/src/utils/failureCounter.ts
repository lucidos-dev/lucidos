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
