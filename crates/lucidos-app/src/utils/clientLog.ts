/** The engine-log breadcrumb channel: a fire-and-forget POST that surfaces as a
 *  `[Client/<category>]` line in engine.log.
 *
 *  Deliberately a LEAF module — it imports only `API` (from utils/basePath, which
 *  imports nothing). `postClientLog` used to live in utils/liveness, whose store
 *  imports make it unusable from low-level utilities: utils/tauri needs to report
 *  a dead IPC bridge (utils/ipcHealth), and routing that through liveness would
 *  put `store/store` in the import graph of every module that calls `invoke`.
 *
 *  utils/liveness re-exports this, so existing `postClientLog` callers are
 *  unaffected. */

import { API } from './basePath';

/** Fire-and-forget POST to the engine's client-log breadcrumb endpoint.
 *  Never throws and never rejects — telemetry must not be able to break a caller
 *  or turn into an unhandled rejection. The engine caps `category`/`message` at
 *  256 chars and the serialized `data` at 4KB (see api/internal.rs) and answers
 *  400 over that; keep payloads small. */
export function postClientLog(category: string, message: string, data: Record<string, unknown>): void {
  try {
    fetch(`${API}/internal/client-log`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ category, message, data }),
      keepalive: true,
    }).catch(() => { /* fire-and-forget */ });
  } catch {
    /* telemetry must never break the app */
  }
}
