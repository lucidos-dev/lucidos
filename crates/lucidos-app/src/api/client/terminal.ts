import { TERMINAL_BUSY_REASON, TERMINAL_DISABLED_REASON } from '@lucidos/engine-constants';
import { API, ApiError, json, mutatingFetch, throwIfNotOk } from './_core';
import { blockedRelayGateway, RelayBlockedError } from '../../utils/socketProbe';

export interface TerminalSize {
  cols: number;
  rows: number;
}

export interface TerminalView extends TerminalSize {
  terminal_id: string;
  attachments: number;
}

export interface TerminalStatus {
  enabled: boolean;
  terminal: TerminalView | null;
}

export interface SocketTicket {
  ticket: string;
  expires_in_secs: number;
}

export type OpenedTerminal = TerminalView & SocketTicket;

function terminalUrl(threadId: string, rest = ''): string {
  return `${API}/threads/${encodeURIComponent(threadId)}/terminal${rest}`;
}

function jsonPost(body: unknown): RequestInit {
  return {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  };
}

export async function openTerminal(threadId: string, size: TerminalSize): Promise<OpenedTerminal> {
  const gateway = await blockedRelayGateway();
  if (gateway) throw new RelayBlockedError(gateway);
  return json<OpenedTerminal>(terminalUrl(threadId), jsonPost(size));
}

export function getTerminalStatus(threadId: string): Promise<TerminalStatus> {
  return json<TerminalStatus>(terminalUrl(threadId));
}

export function mintTerminalTicket(threadId: string, terminalId: string): Promise<SocketTicket> {
  return json<SocketTicket>(
    terminalUrl(threadId, `/${encodeURIComponent(terminalId)}/ticket`),
    jsonPost({}),
  );
}

export async function killTerminal(threadId: string, terminalId: string): Promise<void> {
  const res = await mutatingFetch(terminalUrl(threadId, `/${encodeURIComponent(terminalId)}`), {
    method: 'DELETE',
  });
  await throwIfNotOk(res);
}

export type TypedOutcome = 'typed' | 'busy';

export async function typeTerminalCommand(
  threadId: string,
  terminalId: string,
  command: string,
): Promise<TypedOutcome> {
  const res = await mutatingFetch(
    terminalUrl(threadId, `/${encodeURIComponent(terminalId)}/type`),
    jsonPost({ command }),
  );
  try {
    await throwIfNotOk(res);
    return 'typed';
  } catch (err) {
    if (refusalReason(err) === TERMINAL_BUSY_REASON) return 'busy';
    throw err;
  }
}

function refusalReason(err: unknown): unknown {
  return err instanceof ApiError ? (err.body as { reason?: unknown } | undefined)?.reason : undefined;
}

export function isTerminalDisabled(err: unknown): boolean {
  return refusalReason(err) === TERMINAL_DISABLED_REASON;
}
