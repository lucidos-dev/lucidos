const DEFAULT_MAX_MS = 400;

export function createDblClickGate(maxMs = DEFAULT_MAX_MS) {
  let prev = 0;
  let current = 0;
  return {
    record() {
      prev = current;
      current = Date.now();
    },
    allow(): boolean {
      return prev !== 0 && current - prev <= maxMs;
    },
  };
}
