
import { generateUuid } from './uuid';
import { invoke } from './tauri';

export const HEARTBEAT_INTERVAL_MS = 15_000;

export interface HeartbeatDeps {
  invoke: (cmd: string, args: Record<string, unknown>) => Promise<unknown>;
  doc: Pick<Document, 'visibilityState' | 'addEventListener' | 'removeEventListener'>;
  setInterval: (fn: () => void, ms: number) => number;
  clearInterval: (id: number) => void;
  pageId: string;
}

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

export function startPageHeartbeat(): () => void {
  return startHeartbeat({
    invoke,
    doc: document,
    setInterval: (fn, ms) => window.setInterval(fn, ms),
    clearInterval: (id) => window.clearInterval(id),
    pageId: generateUuid(),
  });
}
