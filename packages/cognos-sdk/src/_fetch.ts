/**
 * Internal fetch wrapper. Single point for:
 * - Base URL resolution
 * - Future auth headers (cognos.configure({ token }))
 * - Timeout handling
 * - Consistent error shape
 */

let _baseUrl = '';
let _authToken: string | undefined;

export function configure(opts: { baseUrl?: string; token?: string }) {
  if (opts.baseUrl !== undefined) _baseUrl = opts.baseUrl;
  if (opts.token !== undefined) _authToken = opts.token;
}

export function getBaseUrl(): string {
  return _baseUrl;
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

async function rawFetch(
  path: string,
  init?: RequestInit,
  timeoutMs = 10000,
): Promise<Response> {
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), timeoutMs);
  const headers: Record<string, string> = {};
  if (_authToken) headers['Authorization'] = `Bearer ${_authToken}`;

  try {
    const res = await fetch(`${_baseUrl}${path}`, {
      ...init,
      signal: controller.signal,
      headers: { ...headers, ...(init?.headers as Record<string, string>) },
    });
    if (!res.ok) {
      let reason = res.statusText;
      try {
        const body = await res.json();
        if (body?.error) reason = body.error;
      } catch { /* body not JSON */ }
      throw new SdkError(res.status, reason);
    }
    return res;
  } finally {
    clearTimeout(timeout);
  }
}

export async function request<T>(
  path: string,
  init?: RequestInit,
  timeoutMs = 10000,
): Promise<T> {
  const res = await rawFetch(path, init, timeoutMs);
  const text = await res.text();
  if (!text) return null as T;
  return JSON.parse(text);
}

export async function requestText(
  path: string,
  init?: RequestInit,
  timeoutMs = 10000,
): Promise<string> {
  const res = await rawFetch(path, init, timeoutMs);
  return res.text();
}

export async function requestVoid(
  path: string,
  init?: RequestInit,
  timeoutMs = 10000,
): Promise<void> {
  await rawFetch(path, init, timeoutMs);
}
