import { retryTransportRead } from './_transport';

import { wsDeviceId } from './_storage';
import { captureDirectRequest, type DirectRequestGuard } from './_restoreGeneration';
import { capabilityCarrier, splitCapability } from './frameCapability';
import {
  callHost,
  fromWireResponse,
  headersToRecord,
  isBridged,
  toWireBody,
  type WireRequest,
  type WireResponse,
} from './_bridge';
import { HEADER_DEVICE_ID } from './generated/engine-constants';

function computeBaseUrl(): string {
  if (typeof document !== 'undefined') {
    const href = document.querySelector('base')?.getAttribute('href');
    if (href) {
      let path = href;
      try {
        if (/^https?:\/\//i.test(href)) path = new URL(href).pathname;
      } catch {
      }
      const capability = splitCapability(path);
      if (capability) return capability.prefix;
      return path.replace(/\/+$/, '');
    }
  }
  const path = (typeof window !== 'undefined' && window.location && window.location.pathname) || '';
  const i = path.indexOf('/app/');
  return i >= 0 ? path.slice(0, i) : '';
}

let _baseUrl = computeBaseUrl();
let _authToken: string | undefined;

const API_V1 = '/api/v1';

export function configure(opts: { baseUrl?: string; token?: string }) {
  if (opts.baseUrl !== undefined) _baseUrl = opts.baseUrl;
  if (opts.token !== undefined) _authToken = opts.token;
}

export function getBaseUrl(): string {
  return _baseUrl;
}

/** Resolve an `/api/v1`-relative suffix to an absolute URL (for `<script src>`,
 *  anchor hrefs, etc.). Pass the path *after* `/api/v1`, e.g.
 *  `apiUrl('/events')` → `<baseUrl>/api/v1/events`.
 *
 *  An app frame can LOAD what this returns, as a subresource, and cannot
 *  `fetch` it: its origin is opaque and CORS refuses. So this is no longer the
 *  escape hatch for an endpoint no SDK method covers. See
 *  `app-frame-escape-hatches` in `docs/temporary-measures.md`. */
export function apiUrl(suffix: string): string {
  const normalized = suffix.startsWith('/') ? suffix : `/${suffix}`;
  return `${_baseUrl}${API_V1}${normalized}`;
}

export function dataMountUrl(path: string): string {
  return `${_baseUrl}${capabilityCarrier()}/data/${path}`;
}

export function apiBase(): string {
  return `${_baseUrl}${API_V1}`;
}

export class SdkError extends Error {
  constructor(
    public readonly httpCode: number,
    public readonly reason: string,
  ) {
    super(`${httpCode} ${reason}`);
    this.name = 'SdkError';
  }
}

export function restampDeadline(
  err: unknown,
  timeoutSignal: AbortSignal,
  timeoutMs: number,
  callerSignal?: AbortSignal | null,
): unknown {
  if (err instanceof DOMException && err.name === 'AbortError'
    && timeoutSignal.aborted && !callerSignal?.aborted) {
    return new DOMException(`Request timed out after ${timeoutMs}ms`, 'TimeoutError');
  }
  return err;
}


async function rawFetch(
  path: string,
  init?: RequestInit,
  timeoutMs = 10000,
  generation = isBridged() ? null : captureDirectRequest(),
): Promise<Response> {
  const normalized = path.startsWith('/') ? path : `/${path}`;
  const res = isBridged()
    ? await bridgedFetch(normalized, init, timeoutMs)
    : await directFetch(normalized, init, timeoutMs, generation!);
  if (!res.ok) {
    let reason = res.statusText;
    try {
      const body = await res.json();
      if (body?.error) reason = body.error;
    } catch {  }
    throw new SdkError(res.status, reason);
  }
  return res;
}

async function directFetch(
  normalized: string,
  init: RequestInit | undefined,
  timeoutMs: number,
  generation: DirectRequestGuard,
): Promise<Response> {
  generation.assertCurrent();
  if (generation.wait) await generation.wait();
  generation.assertCurrent();
  const headers: Record<string, string> = {};
  const deviceId = wsDeviceId();
  if (deviceId) headers[HEADER_DEVICE_ID] = deviceId;
  if (_authToken) headers['Authorization'] = `Bearer ${_authToken}`;

  const timeoutSignal = AbortSignal.timeout(timeoutMs);
  const signal = init?.signal ? AbortSignal.any([init.signal, timeoutSignal]) : timeoutSignal;
  const requestHeaders = new Headers({ ...headers, ...headersToRecord(init?.headers) });
  requestHeaders.set('x-lucidos-restore-generation', generation.header);
  try {
    const response = await retryTransportRead(() => fetch(`${_baseUrl}${API_V1}${normalized}`, {
      ...init,
      signal,
      headers: Object.fromEntries(requestHeaders.entries()),
    }), { ...init, signal }, () => generation.assertCurrent());
    if (generation.wait) await generation.wait();
    generation.assertCurrent();
    return response;
  } catch (err) {
    throw restampDeadline(err, timeoutSignal, timeoutMs, init?.signal);
  }
}

async function bridgedFetch(
  normalized: string,
  init: RequestInit | undefined,
  timeoutMs: number,
): Promise<Response> {
  const headers: Record<string, string> = {};
  if (_authToken) headers['Authorization'] = `Bearer ${_authToken}`;
  const wire: WireRequest = {
    path: normalized,
    method: init?.method ?? 'GET',
    headers: { ...headers, ...headersToRecord(init?.headers) },
    body: toWireBody(init?.body),
    timeoutMs,
  };
  const abort = init?.signal
    ? new Promise<never>((_, reject) => {
      init.signal?.addEventListener('abort', () =>
        reject(new DOMException('Request cancelled', 'AbortError')), { once: true });
    })
    : null;
  const call = callHost('fetch', wire, timeoutMs).then((v) => fromWireResponse(v as WireResponse));
  return abort ? Promise.race([call, abort]) : call;
}

export async function request<T>(
  path: string,
  init?: RequestInit,
  timeoutMs = 10000,
): Promise<T> {
  const guard = isBridged() ? null : captureDirectRequest();
  const res = await rawFetch(path, init, timeoutMs, guard);
  const text = await res.text();
  if (guard?.wait) await guard.wait();
  guard?.assertCurrent();
  if (!text) return null as T;
  return JSON.parse(text);
}

export async function requestText(
  path: string,
  init?: RequestInit,
  timeoutMs = 10000,
): Promise<string> {
  const guard = isBridged() ? null : captureDirectRequest();
  const res = await rawFetch(path, init, timeoutMs, guard);
  const text = await res.text();
  if (guard?.wait) await guard.wait();
  guard?.assertCurrent();
  return text;
}

export async function requestBlob(
  path: string,
  init?: RequestInit,
  timeoutMs = 10000,
): Promise<Blob> {
  const guard = isBridged() ? null : captureDirectRequest();
  const res = await rawFetch(path, init, timeoutMs, guard);
  const blob = await res.blob();
  if (guard?.wait) await guard.wait();
  guard?.assertCurrent();
  return blob;
}

export async function requestVoid(
  path: string,
  init?: RequestInit,
  timeoutMs = 10000,
): Promise<void> {
  await rawFetch(path, init, timeoutMs);
}
