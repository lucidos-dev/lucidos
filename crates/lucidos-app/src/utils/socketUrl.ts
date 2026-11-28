import { QUERY_SOCKET_TICKET } from '@lucidos/engine-constants';
import { API } from './basePath';

export function socketScheme(pageProtocol: string): 'ws:' | 'wss:' {
  return pageProtocol === 'https:' ? 'wss:' : 'ws:';
}

export interface SocketOrigin {
  api: string;
  protocol: string;
  host: string;
}

function socketBase(opts: SocketOrigin): string {
  return `${socketScheme(opts.protocol)}//${opts.host}${opts.api}`;
}

export function computeVoiceSocketUrl(
  opts: SocketOrigin & { threadId: string; ticket: string },
): string {
  const thread = encodeURIComponent(opts.threadId);
  const ticket = encodeURIComponent(opts.ticket);
  return `${socketBase(opts)}/voice?thread_id=${thread}&${QUERY_SOCKET_TICKET}=${ticket}`;
}

export function computeTerminalSocketUrl(
  opts: SocketOrigin & { threadId: string; terminalId: string; ticket: string },
): string {
  const thread = encodeURIComponent(opts.threadId);
  const terminal = encodeURIComponent(opts.terminalId);
  const ticket = encodeURIComponent(opts.ticket);
  return `${socketBase(opts)}/threads/${thread}/terminal/${terminal}/socket?${QUERY_SOCKET_TICKET}=${ticket}`;
}

export function computeWsEchoUrl(opts: SocketOrigin): string {
  return `${socketBase(opts)}/ws-echo`;
}

function servedOrigin(): SocketOrigin {
  return { api: API, protocol: location.protocol, host: location.host };
}

export function voiceSocketUrl(threadId: string, ticket: string): string {
  return computeVoiceSocketUrl({ ...servedOrigin(), threadId, ticket });
}

export function terminalSocketUrl(threadId: string, terminalId: string, ticket: string): string {
  return computeTerminalSocketUrl({ ...servedOrigin(), threadId, terminalId, ticket });
}

export function wsEchoUrl(): string {
  return computeWsEchoUrl(servedOrigin());
}
