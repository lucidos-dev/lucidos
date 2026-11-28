import { REMOTE_GATEWAY } from './basePath';
import { wsEchoUrl } from './socketUrl';

const UPGRADE_PROBE_MS = 3_000;
const PROBE_MESSAGE = 'lucidos';

export function probeUpgrade(): Promise<boolean> {
  return new Promise((resolve) => {
    let socket: WebSocket;
    try {
      socket = new WebSocket(wsEchoUrl());
    } catch {
      resolve(false);
      return;
    }
    const finish = (carried: boolean): void => {
      clearTimeout(timer);
      socket.onopen = null;
      socket.onmessage = null;
      socket.onclose = null;
      socket.onerror = null;
      socket.close();
      resolve(carried);
    };
    const timer = setTimeout(() => finish(false), UPGRADE_PROBE_MS);
    socket.onopen = () => socket.send(PROBE_MESSAGE);
    socket.onmessage = (event) => finish(event.data === PROBE_MESSAGE);
    socket.onclose = () => finish(false);
    socket.onerror = () => finish(false);
  });
}

export async function blockedRelayGateway(): Promise<string | null> {
  if (!REMOTE_GATEWAY || await probeUpgrade()) return null;
  return REMOTE_GATEWAY;
}

export function relayBlockedText(gateway: string): string {
  return `Live connections can't get through the relay to "${gateway}". Open this workspace on "${gateway}" directly, or update both gateways.`;
}

export class RelayBlockedError extends Error {
  constructor(readonly gateway: string) {
    super(relayBlockedText(gateway));
    this.name = 'RelayBlockedError';
  }
}
