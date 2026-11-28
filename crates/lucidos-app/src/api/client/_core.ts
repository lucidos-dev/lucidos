import { isTransportError, retryTransportRead, transportRetryExhausted } from '../../../../../packages/lucidos-sdk/src/_transport';
export { isTransportError, retryTransportRead } from '../../../../../packages/lucidos-sdk/src/_transport';
import { bindSourceGeneration, sourceGeneration } from '../../utils/restoreSourceGeneration';
import { effect } from '@preact/signals';
import { engineRestarting } from '../../store/store';
import { BASE_PATH, REMOTE_GATEWAY, WORKSPACE_ID, workspaceRouteSegment } from '../../utils/basePath';
import { replaceDocument } from '../../utils/documentNavigation';
import { mismatchPickerHref, readWorkspaceClientMismatch } from '../../utils/workspaceClientMismatch';
import { deviceIdHeader } from '../../utils/deviceIdHeader';
import { registrationToAwait } from '../../utils/deviceRegistration';
import { clampText } from '../../utils/clampText';
import { trackRequest } from './requestsInFlight';
import { isProvisionalRead, isRestoreBootstrapRequest, noteProvisionalRead, restoreAdmissionProvisional, restoreAdmissionReady, waitForRestoreAdmission } from '../../utils/restoreAdmission';

export const API_BASE = BASE_PATH;
export { API } from '../../utils/basePath';

export class ApiError extends Error {
  constructor(
    public readonly httpCode: number,
    public readonly reason: string,
    public readonly body?: unknown,
    public readonly bootSplash: boolean = false,
    public readonly workspaceStopped: boolean = false,
  ) {
    super(`${httpCode} ${reason}`);
    this.name = 'ApiError';
  }
}

function awaitEngineReady(): Promise<void> {
  if (!engineRestarting.value) return Promise.resolve();
  return new Promise<void>((resolve) => {
    const dispose = effect(() => {
      if (!engineRestarting.value) {
        dispose();
        resolve();
      }
    });
  });
}

function isHealthProbe(url: string): boolean {
  return url.endsWith('/health');
}

export interface RequestGenerationGuard {
  (): void;
  readonly headers: Readonly<Record<string, string>>;
  readonly gated: boolean;
}

export function requestGenerationGuard(url = '', provisional = false): RequestGenerationGuard {
  const generation = bindSourceGeneration();
  const gated = !provisional && !isRestoreBootstrapRequest(url);
  const check = () => {
    if (gated && sourceGeneration() !== generation()) {
      throw new ApiError(409, 'Workspace restored; review retained work before trying again');
    }
  };
  return Object.defineProperties(check, {
    headers: { get: () => Object.freeze({ 'x-lucidos-restore-generation': generation() ?? 'none' }) },
    gated: { value: gated },
  }) as RequestGenerationGuard;
}

function requestMethod(init: RequestInit | undefined): string {
  return (init?.method ?? 'GET').toUpperCase();
}

function readGenerationGuard(url: string, init: RequestInit | undefined): RequestGenerationGuard {
  const provisional = isProvisionalRead(url, requestMethod(init));
  if (provisional) noteProvisionalRead();
  return requestGenerationGuard(url, provisional);
}

function admissionPending(checkGeneration: RequestGenerationGuard): boolean {
  return checkGeneration.gated && !restoreAdmissionReady();
}

export async function mutatingFetch(url: string, init?: MutationInit, restoreRecovery = false): Promise<Response> {
  return fetchMutation(url, init, restoreRecovery, requestGenerationGuard(url));
}

export interface MutationInit extends RequestInit {
  timeoutMs?: number;
}

async function fetchMutation(url: string, init: MutationInit | undefined, restoreRecovery: boolean, checkGeneration: RequestGenerationGuard): Promise<Response> {
  const { timeoutMs, ...request } = init ?? {};
  checkGeneration();
  if (!restoreRecovery && admissionPending(checkGeneration)) await waitForRestoreAdmission();
  const registration = registrationToAwait(url, request.method);
  if (registration) await registration;
  if (!restoreRecovery && admissionPending(checkGeneration)) await waitForRestoreAdmission();
  checkGeneration();
  const done = trackRequest(url);
  try {
    const headers = { ...deviceIdHeader(), ...Object.fromEntries(new Headers(request.headers).entries()), ...checkGeneration.headers };
    const response = await fetchWithDeadline(url, { ...request, headers }, timeoutMs);
    if (!restoreRecovery && admissionPending(checkGeneration)) await waitForRestoreAdmission();
    checkGeneration();
    return response;
  } finally {
    done();
  }
}

export function isTransientFetchError(err: unknown): boolean {
  if (err instanceof ApiError) return err.bootSplash;
  if (err instanceof DOMException) {
    return err.name === 'AbortError' || err.name === 'TimeoutError';
  }
  return isTransportError(err);
}

export async function retryTransient<T>(request: () => Promise<T>): Promise<T> {
  const checkGeneration = requestGenerationGuard('', restoreAdmissionProvisional());
  try {
    const result = await request();
    checkGeneration();
    return result;
  } catch (err) {
    if (!isTransientFetchError(err) || transportRetryExhausted(err)) throw err;
    checkGeneration();
    const result = await request();
    checkGeneration();
    return result;
  }
}

export async function mutatingFetchIdempotent(url: string, init?: MutationInit): Promise<Response> {
  const checkGeneration = requestGenerationGuard(url);
  try {
    return await fetchMutation(url, init, false, checkGeneration);
  } catch (err) {
    if (isTransportError(err)) return fetchMutation(url, init, false, checkGeneration);
    throw err;
  }
}

const MAX_TEXT_REASON_CHARS = 200;

const RESTARTING_REASON = 'Lucidos is restarting';

function looksLikeMarkup(res: Response, text: string): boolean {
  if (/html|xml/i.test(res.headers.get('content-type') ?? '')) return true;
  return text.trimStart().startsWith('<');
}

const WORKSPACE_STATE_HEADER = 'x-lucidos-workspace-state';
const WORKSPACE_STOPPED = 'stopped';

function isBootSplashResponse(res: Response, text: string): boolean {
  if (res.headers.get('x-lucidos-boot-splash') === '1') return true;
  return res.status === 503 && looksLikeMarkup(res, text);
}

function statusPhrase(res: Response): string {
  if (res.status === 503) return RESTARTING_REASON;
  return res.statusText || `HTTP ${res.status}`;
}

function errorReason(res: Response, text: string, json: unknown): string {
  const obj = json as Record<string, unknown> | null | undefined;
  if (typeof obj?.error === 'string' && obj.error) return obj.error;
  if (typeof obj?.reason === 'string' && obj.reason) return obj.reason;
  if (json === undefined && text && !looksLikeMarkup(res, text)) {
    const firstLine = text.split('\n', 1)[0].replace(/\s+/g, ' ').trim();
    if (firstLine) return clampText(firstLine, MAX_TEXT_REASON_CHARS);
  }
  return statusPhrase(res);
}

let routingToMismatch = false;

export async function throwIfNotOk(res: Response, { routeMismatch = true } = {}): Promise<void> {
  if (res.ok) return;
  const text = await res.text().catch(() => '');
  let json: unknown;
  if (text) {
    try {
      json = JSON.parse(text);
    } catch {  }
  }
  if (routeMismatch && res.status === 426 && REMOTE_GATEWAY && !routingToMismatch) {
    const mismatch = readWorkspaceClientMismatch(json);
    if (mismatch && workspaceRouteSegment(mismatch.route) === WORKSPACE_ID) {
      routingToMismatch = true;
      replaceDocument(mismatchPickerHref(mismatch));
    }
  }
  throw new ApiError(
    res.status,
    errorReason(res, text, json),
    json,
    isBootSplashResponse(res, text),
    res.headers.get(WORKSPACE_STATE_HEADER) === WORKSPACE_STOPPED,
  );
}

export function throwIfRefused<T extends { success: boolean; error?: string }>(
  result: T,
  fallback: string,
): T {
  if (!result.success) throw new Error(result.error || fallback);
  return result;
}

async function fetchWithDefaults(url: string, init: RequestInit | undefined, timeoutMs: number, checkGeneration: RequestGenerationGuard): Promise<Response> {
  if (admissionPending(checkGeneration)) await waitForRestoreAdmission();
  const method = requestMethod(init);
  if (method === 'GET' && !isHealthProbe(url)) await awaitEngineReady();
  const registration = registrationToAwait(url, method);
  if (registration) await registration;
  if (admissionPending(checkGeneration)) await waitForRestoreAdmission();
  checkGeneration();
  const done = trackRequest(url);
  try {
    const request = () => {
      const headers = { ...deviceIdHeader(), ...Object.fromEntries(new Headers(init?.headers).entries()), ...checkGeneration.headers };
      return fetchWithDeadline(url, { ...init, headers }, timeoutMs);
    };
    const response = isHealthProbe(url)
      ? await request()
      : await retryTransportRead(request, init, checkGeneration);
    if (admissionPending(checkGeneration)) await waitForRestoreAdmission();
    checkGeneration();
    return response;
  } finally {
    done();
  }
}

export async function fetchWithDeadline(url: string, init: RequestInit, timeoutMs: number | undefined): Promise<Response> {
  if (timeoutMs === undefined) return fetch(url, init);
  const timeoutSignal = AbortSignal.timeout(timeoutMs);
  const signal = init.signal ? AbortSignal.any([init.signal, timeoutSignal]) : timeoutSignal;
  try {
    return await fetch(url, { ...init, signal });
  } catch (err) {
    if (err instanceof DOMException && err.name === 'AbortError'
      && timeoutSignal.aborted && !init.signal?.aborted) {
      throw new DOMException(`Request timed out after ${timeoutMs}ms`, 'TimeoutError');
    }
    throw err;
  }
}

export async function json<T>(url: string, init?: RequestInit, timeoutMs = 10000): Promise<T> {
  const checkGeneration = readGenerationGuard(url, init);
  const res = await fetchWithDefaults(url, init, timeoutMs, checkGeneration);
  await throwIfNotOk(res);
  return readJsonBody<T>(res, checkGeneration);
}

export type ConditionalRead<T> =
  | { modified: true; body: T; etag: string | null }
  | { modified: false };

export async function conditionalJson<T>(url: string, etag: string | null, timeoutMs = 10000): Promise<ConditionalRead<T>> {
  const init = etag === null ? undefined : { headers: { 'If-None-Match': etag } };
  const checkGeneration = readGenerationGuard(url, init);
  const res = await fetchWithDefaults(url, init, timeoutMs, checkGeneration);
  if (res.status === 304) return { modified: false };
  await throwIfNotOk(res);
  return { modified: true, body: await readJsonBody<T>(res, checkGeneration), etag: res.headers.get('etag') };
}

async function readJsonBody<T>(res: Response, checkGeneration: RequestGenerationGuard): Promise<T> {
  try {
    const body = await res.json() as T;
    if (admissionPending(checkGeneration)) await waitForRestoreAdmission();
    checkGeneration();
    return body;
  } catch (err) {
    if (!(err instanceof SyntaxError)) throw err;
    throw new SyntaxError('The server sent a reply Lucidos could not read');
  }
}

export async function text(url: string, init?: RequestInit, timeoutMs = 10000): Promise<string> {
  const checkGeneration = readGenerationGuard(url, init);
  const res = await fetchWithDefaults(url, init, timeoutMs, checkGeneration);
  await throwIfNotOk(res);
  const body = await res.text();
  if (admissionPending(checkGeneration)) await waitForRestoreAdmission();
  checkGeneration();
  return body;
}
