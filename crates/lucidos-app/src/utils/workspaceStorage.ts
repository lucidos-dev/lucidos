
import { peerPrefix, peerScopedKey } from '../../../../packages/lucidos-sdk/src/_peerScope';
import { SPENT_KEY } from './pairingCodeSeed';
import { REMOTE_WORKSPACES_CACHE_KEY } from './remoteWorkspaces';

import {
  LAST_WORKSPACE_KEY,
  LAST_WORKSPACE_COUNT_KEY,
  WORKSPACE_SWITCHER_EXPANDED_KEY,
} from './lastWorkspace';

const NAMESPACE_PREFIX = 'ws:';

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

export function installWorkspaceStorage(
  storage: Storage,
  ws: string | null,
  peer: string | null = null,
): void {
  if (!ws && !peer) return;

  try {
    if (ws) migrateUnprefixedKeys(storage, ws, peer);

    const proto = Object.getPrototypeOf(storage) as Storage &
      Partial<Record<typeof INSTALLED, boolean>>;
    if (proto[INSTALLED]) return;

    const rawGet = proto.getItem;
    const rawSet = proto.setItem;
    const rawRemove = proto.removeItem;
    const mapKey = (key: string): string => rawStorageKey(key, ws, peer);

    proto.getItem = function (this: Storage, key: string) {
      return rawGet.call(this, mapKey(key));
    };
    proto.setItem = function (this: Storage, key: string, value: string) {
      return rawSet.call(this, mapKey(key), value);
    };
    proto.removeItem = function (this: Storage, key: string) {
      return rawRemove.call(this, mapKey(key));
    };
    Object.defineProperty(proto, INSTALLED, { value: true, configurable: true });
  } catch (err) {
    console.warn('[workspaceStorage] install failed; using raw storage', err);
  }
}
