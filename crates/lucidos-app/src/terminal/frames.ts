import type { TerminalExitCause } from '../generated/thread-event-wire';
import type { TerminalSize } from '../api/client/terminal';

export type ClientFrame =
  | ({ type: 'hello'; resume_from?: number } & TerminalSize)
  | ({ type: 'resize' } & TerminalSize)
  | { type: 'focus' };

export type ServerFrame =
  | { type: 'sync'; terminal_id: string; offset: number; reset: boolean }
  | ({ type: 'size' } & TerminalSize)
  | { type: 'attachments'; count: number }
  | { type: 'exit'; cause: TerminalExitCause; status: number | null };

const SERVER_FRAME_TYPES: ReadonlySet<string> = new Set(['sync', 'size', 'attachments', 'exit']);

export function parseServerFrame(text: string): ServerFrame | null {
  let value: unknown;
  try {
    value = JSON.parse(text);
  } catch {
    return null;
  }
  const type = (value as { type?: unknown } | null)?.type;
  return typeof type === 'string' && SERVER_FRAME_TYPES.has(type) ? value as ServerFrame : null;
}

export function encodeClientFrame(frame: ClientFrame): string {
  return JSON.stringify(frame);
}
