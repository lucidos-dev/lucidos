/**
 * The page's heartbeat to the Tauri crash watchdog (crash_watchdog.rs).
 *
 * Each beat names this document and says whether WebKit shows it. The watchdog
 * judges a reload by the document id, so a late beat from the old document
 * proves nothing. It also leaves a hidden page in a covered window alone,
 * because WebKit suspends such a page and a reload only wakes it for seconds.
 *
 * It beats on the interval and again on `visibilitychange`, so a page that is
 * about to be suspended has already reported hidden.
 *
 * The `catch` is a local no-op, not a swallowed signal: `invoke` reports every
 * failure to the engine log itself (utils/ipcHealth → `[Client/ipc]`). The next
 * beat is 15s away and the Rust watchdog is the recovery path. A toast would be
 * wrong on a timer the user never started.
 */

import { generateUuid } from './uuid';
import { invoke } from './tauri';

export const HEARTBEAT_INTERVAL_MS = 15_000;

export interface HeartbeatDeps {
  invoke: (cmd: string, args: Record<string, unknown>) => Promise<unknown>;
  /** The page's document, which reports `visibilityState`. */
  doc: Pick<Document, 'visibilityState' | 'addEventListener' | 'removeEventListener'>;
  setInterval: (fn: () => void, ms: number) => number;
  clearInterval: (id: number) => void;
  /** One id per document, so the watchdog can tell a reload from a stale beat. */
  pageId: string;
}

/** Start beating, with one beat right away. Returns the stop function. */
export function startHeartbeat(deps: HeartbeatDeps): () => void {
  const beat = () => {
    deps
      .invoke('heartbeat', { page: deps.pageId, visible: deps.doc.visibilityState !== 'hidden' })
      .catch(() => {});
  };
  beat();
  const interval = deps.setInterval(beat, HEARTBEAT_INTERVAL_MS);
  deps.doc.addEventListener('visibilitychange', beat);
  return () => {
    deps.clearInterval(interval);
    deps.doc.removeEventListener('visibilitychange', beat);
  };
}

/** Beat from this document, over the real bridge. */
export function startPageHeartbeat(): () => void {
  return startHeartbeat({
    invoke,
    doc: document,
    setInterval: (fn, ms) => window.setInterval(fn, ms),
    clearInterval: (id) => window.clearInterval(id),
    pageId: generateUuid(),
  });
}
