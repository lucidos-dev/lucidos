import { retryTransportRead } from './_transport';
import { apiUrl } from './_fetch';
import {
  callHost,
  fromWireResponse,
  headersToRecord,
  isBridged,
  toWireBody,
  type WireRequest,
  type WireResponse,
} from './_bridge';
import { PROXY_CLIENT_WAIT_SECS } from './generated/engine-constants';

export interface ProxyClient {
  fetch(path: string, init?: RequestInit): Promise<Response>;
}

const PROXY_TIMEOUT_MS = PROXY_CLIENT_WAIT_SECS * 1000;

export function proxy(name: string): ProxyClient {
  const safeName = encodeURIComponent(name);
  return {
    async fetch(path: string, init?: RequestInit): Promise<Response> {
      const normalizedPath = path.startsWith('/') ? path : `/${path}`;
      const suffix = `/proxy/${safeName}${normalizedPath}`;
      if (!isBridged()) return retryTransportRead(() => fetch(apiUrl(suffix), init), init);
      const wire: WireRequest = {
        path: suffix,
        method: init?.method ?? 'GET',
        headers: headersToRecord(init?.headers),
        body: toWireBody(init?.body),
        timeoutMs: PROXY_TIMEOUT_MS,
      };
      const value = await callHost('fetch', wire, PROXY_TIMEOUT_MS);
      return fromWireResponse(value as WireResponse);
    },
  };
}
