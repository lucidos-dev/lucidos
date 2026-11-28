import type { TerminalExitCause } from '../generated/thread-event-wire';
import type { ConnectionState } from './connection';
import { relayBlockedText } from '../utils/socketProbe';

export type TerminalPhase =
  | { kind: 'checking' }
  | { kind: 'off' }
  | { kind: 'none' }
  | { kind: 'failed'; message: string }
  | { kind: 'relay-blocked'; gateway: string }
  | { kind: 'live'; state: ConnectionState };

const EXIT_CAUSE_TEXT: Record<TerminalExitCause, string> = {
  exited: 'The shell exited.',
  killed: 'It was killed.',
  archived: 'The thread was archived.',
  discarded: 'The thread was discarded.',
  deleted: 'The thread was deleted.',
  'idle-timeout': 'It sat idle too long.',
  disabled: 'Terminal was turned off.',
  'engine-stopped': 'Lucidos restarted.',
};

export function exitCauseText(cause: TerminalExitCause, status: number | null): string {
  if (cause === 'exited' && status !== null && status !== 0) return `The shell exited with status ${status}.`;
  return EXIT_CAUSE_TEXT[cause];
}

function devices(count: number): string {
  return count === 1 ? '1 device' : `${count} devices`;
}

export function terminalStatusText(
  phase: TerminalPhase,
  context: { where: string; attachments: number; mobile: boolean },
): string {
  switch (phase.kind) {
    case 'checking': return '';
    case 'off': return 'Terminal is off.';
    case 'none': return 'No shell is running.';
    case 'failed': return phase.message;
    case 'relay-blocked': return relayBlockedText(phase.gateway);
    case 'live':
      switch (phase.state.kind) {
        case 'connecting': return '';
        case 'reconnecting': return 'Reconnecting…';
        case 'attached': {
          const count = devices(context.attachments);
          return context.mobile
            ? `${context.where} · ${count}`
            : `Running in ${context.where.toLowerCase()} · ${count} attached`;
        }
        case 'ended':
        case 'gone': return 'Shell ended.';
        case 'disabled': return 'Terminal is off.';
      }
  }
}
