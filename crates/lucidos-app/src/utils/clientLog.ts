/** The engine-log breadcrumb channel: a fire-and-forget POST that surfaces as a
 *  `[Client/<category>]` line in the engine's log. Under the gateway that log is
 *  `gateway.log` in the gateway data dir: see `docs/client-logs.md`.
 *
 *  Deliberately a LEAF module: basePath and restoreSourceGeneration have no
 *  runtime store dependencies. `postClientLog` used to live in utils/liveness,
 *  whose store imports make it unusable from low-level utilities: utils/tauri needs to report
 *  a dead IPC bridge (utils/ipcHealth), and routing that through liveness would
 *  put `store/store` in the import graph of every module that calls `invoke`.
 *
 *  utils/liveness re-exports this, so existing `postClientLog` callers are
 *  unaffected. */

import { API } from './basePath';
import { sourceGeneration } from './restoreSourceGeneration';

/** Fire-and-forget POST to the engine's client-log breadcrumb endpoint.
 *  Never throws and never rejects — telemetry must not be able to break a caller
 *  or turn into an unhandled rejection. The engine caps `category`/`message` at
 *  256 chars and the serialized `data` at 4KB (see api/internal.rs) and answers
 *  400 over that; keep payloads small. */
export function postClientLog(category: string, message: string, data: Record<string, unknown>): void {
  try {
    const generation = sourceGeneration();
    fetch(`${API}/internal/client-log`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json', 'x-lucidos-restore-generation': generation ?? 'none' },
      body: JSON.stringify({ category, message, data }),
      keepalive: true,
    }).catch(() => { /* fire-and-forget */ });
  } catch {
    /* telemetry must never break the app */
  }
}
