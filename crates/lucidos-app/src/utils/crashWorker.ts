/** Temporary diagnostics: docs/temporary-measures.md, Page health log. */
const listeners = new Set<(state: number) => void>();
export function noteCrashWorker(state: number): void {
  for (const listener of listeners) {
    try { listener(state); } catch {  }
  }
}
export function onCrashWorker(listener: (state: number) => void): () => void {
  listeners.add(listener);
  return () => { listeners.delete(listener); };
}
