
import { tellHost, type BridgeOp } from './_bridge';

const MAX_TRACE_TEXT_LEN = 200;

export type TraceOp = Extract<BridgeOp, 'log.app-link' | 'log.app-fullscreen'>;

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
  }
}
