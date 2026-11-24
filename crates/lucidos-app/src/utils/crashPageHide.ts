/** Temporary diagnostics: docs/temporary-measures.md, Page health log. */
// The existing liveness pagehide listener supplies the persisted state.
const hideListeners = new Set<(state: number) => void>();
export function noteCrashPageHide(persisted: boolean): void {
  for (const listener of hideListeners) {
    try { listener(persisted ? 2 : 1); } catch {  }
  }
}
export function onCrashPageHide(listener: (state: number) => void): () => void {
  hideListeners.add(listener);
  return () => { hideListeners.delete(listener); };
}
