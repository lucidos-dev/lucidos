
import { getBaseUrl } from './_fetch';
import { callHost, isBridged, tellHost } from './_bridge';
import {
  APP_STORAGE_AREAS,
  APP_STORAGE_SPACES,
  appStoragePrefix,
  type AppStorageArea,
  type AppStorageSnapshot,
  type AppStorageSpace,
} from './appStorage';
import { peerClient, peerScopedKey } from './_peerScope';

function workspaceSlug(): string | null {
  const base = getBaseUrl();
  if (!base) return null;
  const seg = base.replace(/^\/+|\/+$/g, '');
  return seg === '' || seg === '~' ? null : seg;
}

export function inWorkspace(): boolean {
  return workspaceSlug() !== null;
}

function nsKey(key: string): string {
  const slug = workspaceSlug();
  return peerScopedKey(slug ? `ws:${slug}:${key}` : key, peerClient());
}

type Mirror = Record<AppStorageSpace, Record<AppStorageArea, Map<string, string>>>;

function emptyMirror(): Mirror {
  return {
    sdk: { local: new Map(), session: new Map() },
    app: { local: new Map(), session: new Map() },
  };
}

let mirror = emptyMirror();
let priming: Promise<void> | null = null;
let primeSettled = false;
let writtenBeforePrime = new Set<string>();

function noteWrite(space: AppStorageSpace, area: AppStorageArea, key: string | null): void {
  if (primeSettled) return;
  writtenBeforePrime.add(key === null ? `${space}:${area}` : `${space}:${area}:${key}`);
}

export function primeBridgedStorage(): Promise<void> {
  if (!isBridged()) return Promise.resolve();
  priming ??= (callHost('storage.prime', {}) as Promise<AppStorageSnapshot | null>)
    .then((snapshot) => {
      for (const space of APP_STORAGE_SPACES) {
        for (const area of APP_STORAGE_AREAS) {
          if (writtenBeforePrime.has(`${space}:${area}`)) continue;
          for (const [key, value] of Object.entries(snapshot?.[space]?.[area] ?? {})) {
            if (writtenBeforePrime.has(`${space}:${area}:${key}`)) continue;
            mirror[space][area].set(key, value);
          }
        }
      }
    })
    .finally(() => { primeSettled = true; });
  return priming;
}

export function bridgedStorageSettled(): boolean {
  return !isBridged() || primeSettled;
}

function sdkGet(key: string, area: AppStorageArea): string | null {
  return mirror.sdk[area].get(key) ?? null;
}

function sdkSet(key: string, value: string, area: AppStorageArea): void {
  noteWrite('sdk', area, key);
  mirror.sdk[area].set(key, value);
  tellHost('storage.set', { space: 'sdk', area, key, value });
}

function sdkRemove(key: string, area: AppStorageArea): void {
  noteWrite('sdk', area, key);
  mirror.sdk[area].delete(key);
  tellHost('storage.remove', { space: 'sdk', area, key });
}

export function wsLocalGet(key: string): string | null {
  if (isBridged()) return sdkGet(key, 'local');
  try {
    return localStorage.getItem(nsKey(key));
  } catch {
    return null;
  }
}

const DEVICE_ID_KEY = 'lucidos-device-id';

export function wsDeviceId(): string | null {
  return wsLocalGet(DEVICE_ID_KEY);
}

export function wsLocalSet(key: string, value: string): void {
  if (isBridged()) return sdkSet(key, value, 'local');
  try {
    localStorage.setItem(nsKey(key), value);
  } catch {
  }
}

export function wsLocalRemove(key: string): void {
  if (isBridged()) return sdkRemove(key, 'local');
  try {
    localStorage.removeItem(nsKey(key));
  } catch {
  }
}

export function wsSessionGet(key: string): string | null {
  if (isBridged()) return sdkGet(key, 'session');
  try {
    return sessionStorage.getItem(nsKey(key));
  } catch {
    return null;
  }
}

export function wsSessionSet(key: string, value: string): void {
  if (isBridged()) return sdkSet(key, value, 'session');
  sessionStorage.setItem(nsKey(key), value);
}

export function wsSessionRemove(key: string): void {
  if (isBridged()) return sdkRemove(key, 'session');
  sessionStorage.removeItem(nsKey(key));
}

export interface AppStoreBackend {
  entries(): Array<[string, string]>;
  get(key: string): string | null;
  set(key: string, value: string): Promise<void> | void;
  remove(key: string): Promise<void> | void;
  clear(): Promise<void> | void;
}

const lastWrite = new Map<string, number>();
let writeCount = 0;

function writeThrough(
  area: AppStorageArea,
  keys: string[],
  apply: () => void,
  op: 'storage.set' | 'storage.remove' | 'storage.clear',
  args: Record<string, unknown>,
): Promise<void> {
  const map = mirror.app[area];
  const seq = ++writeCount;
  const previous = keys.map((key) => [key, map.get(key)] as const);
  for (const key of keys) lastWrite.set(`${area}:${key}`, seq);
  apply();
  return callHost(op, { space: 'app', area, ...args }).then(
    () => undefined,
    (error: unknown) => {
      for (const [key, value] of previous) {
        if (lastWrite.get(`${area}:${key}`) !== seq) continue;
        if (value === undefined) map.delete(key);
        else map.set(key, value);
      }
      throw error;
    },
  );
}

export function bridgedAppBackend(area: AppStorageArea): AppStoreBackend {
  const map = () => mirror.app[area];
  return {
    entries: () => [...map()],
    get: (key) => map().get(key) ?? null,
    set: (key, value) => {
      noteWrite('app', area, key);
      return writeThrough(area, [key], () => map().set(key, value), 'storage.set', { key, value });
    },
    remove: (key) => {
      noteWrite('app', area, key);
      return writeThrough(area, [key], () => map().delete(key), 'storage.remove', { key });
    },
    clear: () => {
      noteWrite('app', area, null);
      return writeThrough(area, [...map().keys()], () => map().clear(), 'storage.clear', {});
    },
  };
}

export function directAppBackend(area: AppStorageArea, appId: string): AppStoreBackend {
  const prefix = nsKey(appStoragePrefix(appId, 'app'));
  const store = (): Storage => (area === 'session' ? sessionStorage : localStorage);
  const entries = (): Array<[string, string]> => {
    const s = store();
    const out: Array<[string, string]> = [];
    for (let i = 0; i < s.length; i++) {
      const raw = s.key(i);
      if (!raw?.startsWith(prefix)) continue;
      const value = s.getItem(raw);
      if (value !== null) out.push([raw.slice(prefix.length), value]);
    }
    return out;
  };
  return {
    entries,
    get: (key) => store().getItem(prefix + key),
    set: (key, value) => store().setItem(prefix + key, value),
    remove: (key) => store().removeItem(prefix + key),
    clear: () => {
      for (const [key] of entries()) store().removeItem(prefix + key);
    },
  };
}

export function _resetBridgedStorageForTesting(): void {
  mirror = emptyMirror();
  priming = null;
  primeSettled = false;
  writtenBeforePrime = new Set();
  lastWrite.clear();
}
