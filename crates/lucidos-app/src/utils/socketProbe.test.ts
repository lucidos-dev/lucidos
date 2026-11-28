import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { blockedRelayGateway, relayBlockedText } from './socketProbe';

vi.mock('./basePath', () => ({ API: '/sample@peer/api/v1', REMOTE_GATEWAY: 'peer' }));

class ProbeSocket {
  static last: ProbeSocket;
  onopen: (() => void) | null = null;
  onmessage: ((event: MessageEvent) => void) | null = null;
  onclose: (() => void) | null = null;
  onerror: (() => void) | null = null;
  send = vi.fn();
  close = vi.fn();

  constructor(readonly url: string) {
    ProbeSocket.last = this;
  }
}

beforeEach(() => {
  vi.stubGlobal('location', { protocol: 'https:', host: 'gateway.example.test' });
  vi.stubGlobal('WebSocket', ProbeSocket);
});

afterEach(() => vi.unstubAllGlobals());

it('checks that the relay carries a message both ways', async () => {
  const result = blockedRelayGateway();
  const socket = ProbeSocket.last;
  expect(socket.url).toBe('wss://gateway.example.test/sample@peer/api/v1/ws-echo');
  socket.onopen?.();
  socket.onmessage?.(new MessageEvent('message', { data: socket.send.mock.calls[0][0] }));
  await expect(result).resolves.toBeNull();
  expect(socket.close).toHaveBeenCalledOnce();
});

it('names the gateway when the relay accepts the handshake and closes before the echo', async () => {
  const result = blockedRelayGateway();
  ProbeSocket.last.onopen?.();
  ProbeSocket.last.onclose?.();
  await expect(result).resolves.toBe('peer');
});

it('tells the user what to do, naming the gateway', () => {
  const text = relayBlockedText('peer');
  expect(text).toContain('Open this workspace on "peer" directly');
  expect(text).toContain('update both gateways');
});
