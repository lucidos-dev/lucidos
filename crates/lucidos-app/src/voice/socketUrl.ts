/**
 * The address of a call.
 *
 * A `WebSocket` will not take a relative URL, so this is the one place in the
 * voice client that builds an absolute one. It is still base-path aware (ADR
 * 0014). `API` already carries the `/<slug>` prefix the gateway routes on. So a
 * call reaches the right engine from a workspace, the picker, or a legacy
 * direct engine alike.
 *
 * No credential is appended. The gateway authenticates a same-origin handshake
 * by cookie, exactly as it does an ordinary request (ADR 0151).
 */
import { API } from '../utils/basePath';

/** The socket scheme matching a page scheme. */
export function socketScheme(pageProtocol: string): 'ws:' | 'wss:' {
  return pageProtocol === 'https:' ? 'wss:' : 'ws:';
}

/** Where a socket on this origin starts, base path and all. */
export interface SocketOrigin {
  api: string;
  protocol: string;
  host: string;
}

function socketBase(opts: SocketOrigin): string {
  return `${socketScheme(opts.protocol)}//${opts.host}${opts.api}`;
}

/** Pure: the call URL for a thread, given where the page is served from. */
export function computeVoiceSocketUrl(opts: SocketOrigin & { threadId: string }): string {
  return `${socketBase(opts)}/voice?thread_id=${encodeURIComponent(opts.threadId)}`;
}

/**
 * Pure: the echo URL, which answers whether an upgrade survives the hops.
 *
 * The engine's `/api/v1/ws-echo` reaches nothing and holds no session. So a
 * `101` from it means every hop carried the upgrade. That is the one question
 * a refused call cannot answer for itself.
 */
export function computeWsEchoUrl(opts: SocketOrigin): string {
  return `${socketBase(opts)}/ws-echo`;
}

function servedOrigin(): SocketOrigin {
  return { api: API, protocol: location.protocol, host: location.host };
}

/** Live-wired {@link computeVoiceSocketUrl} for the served context. */
export function voiceSocketUrl(threadId: string): string {
  return computeVoiceSocketUrl({ ...servedOrigin(), threadId });
}

/** Live-wired {@link computeWsEchoUrl} for the served context. */
export function wsEchoUrl(): string {
  return computeWsEchoUrl(servedOrigin());
}
