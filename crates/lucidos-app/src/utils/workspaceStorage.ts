
import { peerPrefix, peerScopedKey } from '../../../../packages/lucidos-sdk/src/_peerScope';
import type { AppStorageArea } from '@lucidos/sdk';
import { postClientLog } from './clientLog';
import { SPENT_KEY } from './pairingCodeSeed';
import { REMOTE_WORKSPACES_CACHE_KEY } from './remoteWorkspaces';

import {
  LAST_WORKSPACE_KEY,
  LAST_WORKSPACE_COUNT_KEY,
  WORKSPACE_SWITCHER_EXPANDED_KEY,
} from './lastWorkspace';

const NAMESPACE_PREFIX = 'ws:';

const APP_STORAGE_ROOT = 'appbridge:';

const MIGRATION_MARKER = '__migrated';

export const GLOBAL_KEYS: ReadonlySet<string> = new Set([
  LAST_WORKSPACE_KEY,
  LAST_WORKSPACE_COUNT_KEY,
  WORKSPACE_SWITCHER_EXPANDED_KEY,
  REMOTE_WORKSPACES_CACHE_KEY,
  SPENT_KEY,
]);

const APPEARANCE_SEED_KEYS: readonly string[] = [
  'lucidos-theme',
  'lucidos-font-family',
  'lucidos-ui-scale',
  'lucidos-animation-speed-slider',
];

export function isGlobalKey(key: string): boolean {
  return GLOBAL_KEYS.has(key);
}

export function namespacedKey(key: string, ws: string): string {
  return `${NAMESPACE_PREFIX}${ws}:${key}`;
}

export function rawStorageKey(key: string, ws: string | null, peer: string | null = null): string {
  const prefix = peerPrefix(peer);
  const bare = key.startsWith(prefix) ? key.slice(prefix.length) : key;
  const namespaced =
    !ws || bare.startsWith(NAMESPACE_PREFIX) || isGlobalKey(bare) ? bare : namespacedKey(bare, ws);
  return peerScopedKey(namespaced, peer);
}

export function scopedKeys(storage: Storage, ws: string | null, peer: string | null): string[] {
  const prefix = rawStorageKey('', ws, peer);
  const keys: string[] = [];
  for (let index = 0; index < storage.length; index++) {
    const raw = storage.key(index);
    if (raw === null || !raw.startsWith(prefix)) continue;
    const key = raw.slice(prefix.length);
    if (!key.startsWith(NAMESPACE_PREFIX) && !key.startsWith('peer:') && !key.startsWith('preview:')) keys.push(key);
  }
  return keys;
}

let installedScope: { ws: string | null; peer: string | null } = { ws: null, peer: null };

export function shellStorageKeys(storage: Storage): string[] {
  return scopedKeys(storage, installedScope.ws, installedScope.peer)
    .filter((key) => !key.startsWith(APP_STORAGE_ROOT));
}

export function shouldSeed(key: string): boolean {
  if (key.startsWith(NAMESPACE_PREFIX)) return false;
  return APPEARANCE_SEED_KEYS.includes(key);
}

export function migrateUnprefixedKeys(storage: Storage, ws: string, peer: string | null = null): void {
  const marker = peerScopedKey(namespacedKey(MIGRATION_MARKER, ws), peer);
  if (storage.getItem(marker) !== null) return;

  for (const k of APPEARANCE_SEED_KEYS) {
    if (!shouldSeed(k)) continue;
    const target = peerScopedKey(namespacedKey(k, ws), peer);
    if (storage.getItem(target) !== null) continue;
    const val = storage.getItem(peerScopedKey(k, peer));
    if (val !== null) storage.setItem(target, val);
  }

  storage.setItem(marker, '1');
}

const INSTALLED = Symbol.for('lucidos.workspaceStorage.installed');

const rawReaders = new WeakMap<object, Storage['getItem']>();

const rawWriters = new WeakMap<object, Storage['setItem']>();

export function markSplashQuiet(destinationSlug?: string): void {
  const key = 'lucidos-splash-quiet';
  try {
    const storage = sessionStorage;
    if (destinationSlug === undefined) {
      storage.setItem(key, '1');
      return;
    }
    const write = rawWriters.get(Object.getPrototypeOf(storage)) ?? storage.setItem;
    write.call(storage, namespacedKey(key, destinationSlug), '1');
  } catch {
  }
}

export function storageUsage(storage: Storage) {
  const read = rawReaders.get(Object.getPrototypeOf(storage)) ?? storage.getItem;
  const usage = {
    app: { entries: 0, codeUnits: 0 },
    sdk: { entries: 0, codeUnits: 0 },
    shell: { entries: 0, codeUnits: 0 },
    other: { entries: 0, codeUnits: 0 },
  };
  for (let index = 0; index < storage.length; index++) {
    const key = storage.key(index);
    if (key === null) continue;
    const value = read.call(storage, key);
    if (value === null) continue;
    const name = key.replace(/^(?:peer:[^:]+:)?(?:ws:[^:]+:)?/, '');
    const appSpace = name.startsWith(APP_STORAGE_ROOT)
      ? /^[^:]+:(app|sdk):/.exec(name.slice(APP_STORAGE_ROOT.length))?.[1]
      : undefined;
    const bucket = appSpace === 'app' ? usage.app
      : appSpace === 'sdk' ? usage.sdk
      : name.startsWith('lucidos-') ? usage.shell : usage.other;
    bucket.entries++;
    bucket.codeUnits += key.length + value.length;
  }
  return usage;
}

const SAMPLE_INTERVAL_MS = 60_000;
const lastSample: Partial<Record<AppStorageArea, number>> = {};

export function isStorageQuotaError(error: unknown): boolean {
  return error instanceof DOMException && error.name === 'QuotaExceededError';
}

export function reportStoragePressure(area: AppStorageArea, storage: Storage): void {
  const now = performance.now();
  const previous = lastSample[area];
  if (previous !== undefined && now - previous < SAMPLE_INTERVAL_MS) return;
  lastSample[area] = now;
  try {
    postClientLog('storage-pressure', 'quota', { area, usage: storageUsage(storage) });
  } catch {
    postClientLog('storage-pressure', 'quota', { area, inventoryUnavailable: true });
  }
}

export function installWorkspaceStorage(
  localStore: Storage,
  ws: string | null,
  peer: string | null = null,
): void {
  installedScope = { ws, peer };
  if (!ws && !peerPrefix(peer)) return;

  if (ws) {
    try {
      migrateUnprefixedKeys(localStore, ws, peer);
    } catch (err) {
      if (isStorageQuotaError(err)) reportStoragePressure('local', localStore);
      console.warn('[workspaceStorage] appearance migration failed', err);
    }
  }

  try {
    const proto = Object.getPrototypeOf(localStore) as Storage &
      Partial<Record<typeof INSTALLED, boolean>>;
    if (proto[INSTALLED]) return;

    const rawGet = proto.getItem;
    rawReaders.set(proto, rawGet);
    const rawSet = proto.setItem;
    rawWriters.set(proto, rawSet);
    const rawRemove = proto.removeItem;
    const mapKey = (key: string): string => rawStorageKey(key, ws, peer);

    proto.getItem = function (this: Storage, key: string) {
      return rawGet.call(this, mapKey(key));
    };
    proto.setItem = function (this: Storage, key: string, value: string) {
      try {
        return rawSet.call(this, mapKey(key), value);
      } catch (err) {
        if (isStorageQuotaError(err)) {
          reportStoragePressure(this === localStore ? 'local' : 'session', this);
        }
        throw err;
      }
    };
    proto.removeItem = function (this: Storage, key: string) {
      return rawRemove.call(this, mapKey(key));
    };
    Object.defineProperty(proto, INSTALLED, { value: true, configurable: true });
  } catch (err) {
    console.warn('[workspaceStorage] install failed; using raw storage', err);
  }
}
