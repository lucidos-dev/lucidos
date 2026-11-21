/** Temporary diagnostics: docs/temporary-measures.md, Page health log. */
// The existing liveness pagehide listener supplies the persisted state.
const hideListeners = new Set<(state: number) => void>();
export function noteCrashPageHide(persisted: boolean): void {
  for (const listener of hideListeners) {
    // Diagnostics retry on the next event. A missing mark needs no user action.
    try { listener(persisted ? 2 : 1); } catch { /* Best-effort telemetry. */ }
  }
}
export function onCrashPageHide(listener: (state: number) => void): () => void {
  hideListeners.add(listener);
  return () => { hideListeners.delete(listener); };
}
