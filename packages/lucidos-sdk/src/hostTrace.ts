// Breadcrumbs from an app frame to the engine log. A frame has no log of its
// own that anyone can read on a phone, so the host writes the line for it.

import { tellHost, type BridgeOp } from './_bridge';

/** Longest text a breadcrumb carries for one value. The engine refuses a whole
 *  client log over 4 KB, and a `data:` href can be megabytes. */
const MAX_TRACE_TEXT_LEN = 200;

/** The bridge ops that carry a breadcrumb. */
export type TraceOp = Extract<BridgeOp, 'log.app-link' | 'log.app-fullscreen'>;

/** Send one breadcrumb. Fire-and-forget: it runs after the user's own work
 *  and never throws into it. */
export function traceToHost(op: TraceOp, step: string, detail: Record<string, unknown> = {}): void {
  try {
    const shortened: Record<string, unknown> = {};
    for (const [key, value] of Object.entries(detail)) {
      shortened[key] = typeof value === 'string' && value.length > MAX_TRACE_TEXT_LEN
        ? `${value.slice(0, MAX_TRACE_TEXT_LEN)}…`
        : value;
    }
    tellHost(op, { step, ...shortened });
  } catch {
    // Telemetry must never break the caller.
  }
}
