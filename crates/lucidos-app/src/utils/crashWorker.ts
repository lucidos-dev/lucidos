/** Temporary diagnostics: docs/temporary-measures.md, Page health log. */
const listeners = new Set<(state: number) => void>();
export function noteCrashWorker(state: number): void {
  for (const listener of listeners) {
    // Diagnostics retry on the next event. A missing mark needs no user action.
    try { listener(state); } catch { /* Best-effort telemetry. */ }
  }
}
export function onCrashWorker(listener: (state: number) => void): () => void {
  listeners.add(listener);
  return () => { listeners.delete(listener); };
}
