import { retryTransportRead } from '../../../../../packages/lucidos-sdk/src/_transport';
import { subscribeAppEvents, updateAppEventTypes, unsubscribeAppEvents, clearAppEventSubscribers, resetAppEventStream, postToFrame } from './app-frame-channel';
export { askFrame, tellFrame, postToAppFrame, fanOutEventFrame, fanOutEventStreamStatus, frameEventTypes } from './app-frame-channel';

import {
  APP_STORAGE_AREAS,
  APP_STORAGE_SPACES,
  appMayCall,
  appStoragePrefix,
  appStorageRefusal,
  emptyAppStorageSnapshot,
  normalizeSuffix,
  type AppStorageArea,
  type AppStorageSnapshot,
  type AppStorageSpace,
} from '@lucidos/sdk';
import { API } from '../../api/client';
import { requestGenerationGuard, type RequestGenerationGuard } from '../../api/client/_core';
import { restoreAdmissionReady, waitForRestoreAdmission } from '../../utils/restoreAdmission';
import { postClientLog } from '../../utils/clientLog';
import { deviceIdHeader, readDeviceId } from '../../utils/deviceIdHeader';
import { registrationToAwait } from '../../utils/deviceRegistration';
import { appFrameFor, appIdForFrame } from '../../utils/appFrame';
import { peerClient } from '../../../../../packages/lucidos-sdk/src/_peerScope';
import { WORKSPACE_ID } from '../../utils/basePath';
import { isStorageQuotaError, rawStorageKey, reportStoragePressure } from '../../utils/workspaceStorage';
import { focusedThreadId, showToast } from '../store';
import { BRIDGED_FETCH_SLOTS, FetchLanes, type LanesSnapshot } from './bridgedFetchLanes';
import {
  BRIDGE_REPLY_TYPE,
  BRIDGE_TYPE,
  type BridgeOp,
  type WireBody,
  type WireRequest,
} from '../../../../../packages/lucidos-sdk/src/_bridge';

export function resolveBridgePath(
  path: string,
  method = 'GET',
  appId: string | null = null,
): string | null {
  const resolved = normalizeSuffix(path);
  if (!resolved) return null;
  return appMayCall(method, resolved.pathname, appId) ? resolved.full : null;
}

export function pathIsAllowed(path: string, method = 'GET'): boolean {
  return resolveBridgePath(path, method) !== null;
}

export const THIS_DEVICE = '@device';

function stampDeviceScope(path: string): string {
  const url = new URL(path, 'https://app.invalid');
  if (url.pathname !== '/preferences') return path;
  if (url.searchParams.get('device_id') !== THIS_DEVICE) return path;
  const id = readDeviceId();
  if (id) url.searchParams.set('device_id', id);
  else url.searchParams.delete('device_id');
  return `${url.pathname}${url.search}${url.hash}`;
}

function sanitizeHeaders(headers: Record<string, string> | undefined): Record<string, string> {
  const out: Record<string, string> = {};
  for (const [name, value] of Object.entries(headers ?? {})) {
    if (name.toLowerCase().startsWith('x-lucidos-')) continue;
    out[name] = value;
  }
  return out;
}

function fromWireBody(body: WireBody): BodyInit | null {
  switch (body?.kind) {
    case 'text': return body.value;
    case 'binary': return body.value;
    case 'form': {
      const form = new FormData();
      for (const [name, value] of body.entries) form.append(name, value);
      return form;
    }
    default: return null;
  }
}

function headersToRecord(headers: Headers): Record<string, string> {
  const out: Record<string, string> = {};
  headers.forEach((value, name) => { out[name] = value; });
  return out;
}

export const APP_ID_HEADER = 'x-lucidos-app-id';

const DEFAULT_FETCH_TIMEOUT_MS = 30000;

const USER_INTENT_PREFIX = '/ui/';

function admittedPath(wire: WireRequest, appId: string | null): string {
  const method = wire.method || 'GET';
  const resolved = resolveBridgePath(wire.path, method, appId);
  if (resolved === null) {
    throw new Error(`The app bridge does not carry ${method} ${wire.path}`);
  }
  return stampDeviceScope(resolved);
}

async function sendBridgedFetch(
  wire: WireRequest,
  path: string,
  appId: string | null,
  timeoutMs: number,
  checkGeneration: RequestGenerationGuard,
): Promise<unknown> {
  checkGeneration();
  if (!restoreAdmissionReady()) await waitForRestoreAdmission();
  const method = wire.method || 'GET';
  const registration = registrationToAwait(`${API}${path}`, method);
  if (registration) await registration;
  if (!restoreAdmissionReady()) await waitForRestoreAdmission();
  checkGeneration();
  const res = await retryTransportRead(() => fetch(`${API}${path}`, {
    method,
    headers: {
      ...sanitizeHeaders(wire.headers),
      ...deviceIdHeader(),
      ...checkGeneration.headers,
      ...(appId ? { [APP_ID_HEADER]: appId } : {}),
    },
    body: fromWireBody(wire.body),
    signal: AbortSignal.timeout(timeoutMs),
  }), { method }, checkGeneration);
  if (!restoreAdmissionReady()) await waitForRestoreAdmission();
  checkGeneration();
  const body = await res.arrayBuffer();
  if (!restoreAdmissionReady()) await waitForRestoreAdmission();
  checkGeneration();
  return {
    status: res.status,
    statusText: res.statusText,
    headers: headersToRecord(res.headers),
    body,
  };
}

let fetchLanes = new FetchLanes<HTMLIFrameElement>(BRIDGED_FETCH_SLOTS);

export function bridgedFetchSnapshot(): LanesSnapshot {
  return fetchLanes.snapshot();
}

function bridgedFetch(
  wire: WireRequest,
  appId: string | null,
  frame: HTMLIFrameElement,
): Promise<unknown> {
  const path = admittedPath(wire, appId);
  const checkGeneration = requestGenerationGuard(`${API}${path}`);
  const timeoutMs = wire.timeoutMs || DEFAULT_FETCH_TIMEOUT_MS;
  if (path.startsWith(USER_INTENT_PREFIX)) return sendBridgedFetch(wire, path, appId, timeoutMs, checkGeneration);
  return fetchLanes.enqueue(frame, Date.now() + timeoutMs, (remainingMs) =>
    sendBridgedFetch(wire, path, appId, remainingMs, checkGeneration),
  );
}


interface StorageArgs {
  space?: unknown;
  area?: unknown;
  key?: unknown;
  value?: unknown;
}

interface StorageTarget {
  space: AppStorageSpace;
  area: AppStorageArea;
}

class BrowserStorageError extends Error {
  constructor(readonly reason: 'full' | 'unavailable') {
    super(reason === 'full' ? 'Browser storage is full' : 'Browser storage is unavailable here');
  }
}

function storageTarget(args: StorageArgs): StorageTarget {
  const { space, area } = args;
  if (!APP_STORAGE_SPACES.includes(space as AppStorageSpace)) {
    throw new Error(`App storage has no space "${String(space)}"`);
  }
  if (!APP_STORAGE_AREAS.includes(area as AppStorageArea)) {
    throw new Error(`App storage has no area "${String(area)}"`);
  }
  return { space: space as AppStorageSpace, area: area as AppStorageArea };
}

function storageString(args: StorageArgs, name: 'key' | 'value'): string {
  const value = args[name];
  if (typeof value !== 'string') throw new Error(`App storage needs a string ${name}`);
  return value;
}

function browserStore(area: AppStorageArea): Storage {
  try {
    const store = area === 'session' ? sessionStorage : localStorage;
    if (store) return store;
  } catch {
  }
  throw new BrowserStorageError('unavailable');
}

function storedPrefix(appId: string, space: AppStorageSpace): string {
  return rawStorageKey(appStoragePrefix(appId, space), WORKSPACE_ID, peerClient());
}

function entriesUnder(store: Storage, prefix: string): Array<[string, string]> {
  const out: Array<[string, string]> = [];
  for (let i = 0; i < store.length; i++) {
    const raw = store.key(i);
    if (!raw?.startsWith(prefix)) continue;
    const value = store.getItem(raw);
    if (value !== null) out.push([raw.slice(prefix.length), value]);
  }
  return out;
}

function storagePrime(appId: string): AppStorageSnapshot {
  const snapshot = emptyAppStorageSnapshot();
  for (const area of APP_STORAGE_AREAS) {
    let store: Storage;
    try {
      store = browserStore(area);
    } catch {
      continue;
    }
    for (const space of APP_STORAGE_SPACES) {
      snapshot[space][area] = Object.fromEntries(entriesUnder(store, storedPrefix(appId, space)));
    }
  }
  return snapshot;
}

function storageSet(appId: string, args: StorageArgs): null {
  const { space, area } = storageTarget(args);
  const key = storageString(args, 'key');
  const value = storageString(args, 'value');
  const store = browserStore(area);
  const prefix = storedPrefix(appId, space);
  const refusal = appStorageRefusal(entriesUnder(store, prefix), key, value, space);
  if (refusal) throw new Error(refusal);
  try {
    store.setItem(prefix + key, value);
  } catch (error) {
    if (isStorageQuotaError(error)) {
      reportStoragePressure(area, store);
      throw new BrowserStorageError('full');
    }
    if (error instanceof DOMException && error.name === 'SecurityError') {
      throw new BrowserStorageError('unavailable');
    }
    throw error;
  }
  return null;
}

function storageRemove(appId: string, args: StorageArgs): null {
  const { space, area } = storageTarget(args);
  browserStore(area).removeItem(storedPrefix(appId, space) + storageString(args, 'key'));
  return null;
}

function removeUnder(store: Storage, prefix: string): void {
  for (const [key] of entriesUnder(store, prefix)) store.removeItem(prefix + key);
}

function storageClear(appId: string, args: StorageArgs): null {
  const { space, area } = storageTarget(args);
  removeUnder(browserStore(area), storedPrefix(appId, space));
  return null;
}

export function clearAppStorage(appId: string): void {
  let prefixes: string[];
  try {
    prefixes = APP_STORAGE_SPACES.map((space) => storedPrefix(appId, space));
  } catch {
    return;
  }
  for (const area of APP_STORAGE_AREAS) {
    let store: Storage;
    try {
      store = browserStore(area);
    } catch {
      continue;
    }
    for (const prefix of prefixes) removeUnder(store, prefix);
  }
}

function runStorageOp(
  frame: HTMLIFrameElement,
  args: unknown,
  op: (appId: string, args: StorageArgs) => unknown,
): unknown {
  const appId = appIdForFrame(frame);
  if (!appId) throw new Error('This frame runs no app, so it has no storage');
  const storageArgs = (args ?? {}) as StorageArgs;
  try {
    return op(appId, storageArgs);
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    if (storageArgs.space === 'app') {
      showToast(storageFailureMessage(error, message, storageArgs.area), 'error', {
        title: `App "${appId}" could not save`,
        key: `app-storage:${appId}`,
      });
    } else {
      console.warn(`[app-bridge] app "${appId}" could not keep its scroll position:`, message);
    }
    throw error;
  }
}

function storageFailureMessage(error: unknown, message: string, area: unknown): string {
  if (!(error instanceof BrowserStorageError)) {
    return `If this keeps happening, report it to the app author. ${message}`;
  }
  if (error.reason === 'full') {
    if (area === 'session') {
      return 'Browser storage is full for this tab. Copy any unsaved work, then close this tab, reopen Lucidos, and try again.';
    }
    return 'Browser storage is full. Open Lucidos in another browser or on another device. Clearing Lucidos site data can remove drafts and app settings on this device.';
  }
  return 'Browser storage is unavailable. Allow site data for Lucidos in your browser settings, or try another browser or device.';
}

const MAX_BREADCRUMB_DETAIL_BYTES = 2000;

function logAppBreadcrumb(category: 'app-link' | 'app-fullscreen', args: unknown, appId: string | null): void {
  const { step, ...detail } = (args && typeof args === 'object' ? args : {}) as Record<string, unknown>;
  const fits = new TextEncoder().encode(JSON.stringify(detail)).length <= MAX_BREADCRUMB_DETAIL_BYTES;
  postClientLog(category, typeof step === 'string' ? step.slice(0, 64) : 'unknown', {
    ...(fits ? detail : { detailDropped: true }),
    app: appId,
  });
}


async function runOp(
  op: BridgeOp,
  args: unknown,
  source: MessageEventSource,
  frame: HTMLIFrameElement,
): Promise<unknown> {
  switch (op) {
    case 'media.poster': {
      const { mediaPoster } = await import('./app-media-bridge');
      return mediaPoster(args, frame);
    }
    case 'fetch': return bridgedFetch(args as WireRequest, appIdForFrame(frame), frame);
    case 'sse.open':
      subscribeAppEvents(source, frame, args);
      return null;
    case 'sse.types': {
      updateAppEventTypes(source, args);
      return null;
    }
    case 'sse.close': unsubscribeAppEvents(source); return null;
    case 'storage.prime': return runStorageOp(frame, args, storagePrime);
    case 'storage.set': return runStorageOp(frame, args, storageSet);
    case 'storage.remove': return runStorageOp(frame, args, storageRemove);
    case 'storage.clear': return runStorageOp(frame, args, storageClear);
    case 'ui.focused-thread': return focusedThreadId.value;
    case 'log.app-link': logAppBreadcrumb('app-link', args, appIdForFrame(frame)); return null;
    case 'log.app-fullscreen': logAppBreadcrumb('app-fullscreen', args, appIdForFrame(frame)); return null;
    default: throw new Error(`The app bridge has no operation "${op}"`);
  }
}

export function installAppBridge(): () => void {
  const onMessage = (event: MessageEvent) => {
    const data = event.data as { type?: string; id?: string; op?: string; args?: unknown } | null;
    if (!data || typeof data !== 'object' || data.type !== BRIDGE_TYPE) return;
    const source = event.source;
    const frame = appFrameFor(source);
    if (!source || !frame) return;
    if (typeof data.op !== 'string') return;
    void runOp(data.op as BridgeOp, data.args, source, frame).then(
      (value) => {
        if (!data.id) return;
        postToFrame(source, { type: BRIDGE_REPLY_TYPE, id: data.id, ok: true, value });
      },
      (error: unknown) => {
        if (!data.id) return;
        const message = error instanceof Error ? error.message : String(error);
        postToFrame(source, { type: BRIDGE_REPLY_TYPE, id: data.id, ok: false, error: message });
      },
    );
  };
  window.addEventListener('message', onMessage);
  return () => {
    window.removeEventListener('message', onMessage);
    clearAppEventSubscribers();
  };
}

export function _resetAppBridgeForTesting(): void {
  clearAppEventSubscribers();
  fetchLanes = new FetchLanes(BRIDGED_FETCH_SLOTS);
  resetAppEventStream();
}
