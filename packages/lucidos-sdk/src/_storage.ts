/**
 * Browser storage for the SDK (iframe realm): per-workspace keys, and the
 * store behind `lucidos.storage`.
 *
 * The SDK is a SEPARATE JS realm from the host app, so the host's
 * `Storage.prototype` override
 * (`crates/lucidos-app/src/utils/workspaceStorage.ts`) does NOT apply here.
 * Every storage key the SDK touches is per-workspace (theme, fonts, scale,
 * device id, per-app scroll), so it must be read/written under
 * `ws:<slug>:<key>`, the SAME namespace the host writes. Otherwise a host write
 * won't match a standalone app tab's read. An app FRAME reaches no host key at
 * all: its store is the host's `appbridge:` space, primed over the bridge.
 *
 * The slug comes from the SDK base path (`_fetch.ts` `getBaseUrl()`):
 * `/<slug>` behind the gateway, `''` at a legacy direct-engine root, `/~` in the
 * picker (no app iframes there). No slug → raw key (legacy/direct, where there
 * is a single workspace per origin and nothing to isolate).
 *
 * This is the ONLY SDK file allowed to touch `localStorage` / `sessionStorage`
 * directly — the `no-raw-storage` guard test fails the build on any other raw
 * access in the SDK.
 */

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

/** The workspace slug this iframe runs under, or null (picker / legacy root). */
function workspaceSlug(): string | null {
  const base = getBaseUrl(); // '' | '/<slug>' | '/~'
  if (!base) return null;
  const seg = base.replace(/^\/+|\/+$/g, '');
  return seg === '' || seg === '~' ? null : seg;
}

/** Whether this realm runs inside a workspace, so its keys are namespaced. */
export function inWorkspace(): boolean {
  return workspaceSlug() !== null;
}

/** `ws:<slug>:<key>` in a workspace, else the raw key (picker / legacy). */
function nsKey(key: string): string {
  const slug = workspaceSlug();
  return slug ? `ws:${slug}:${key}` : key;
}

/**
 * What the host holds for this frame, when the frame cannot read storage.
 *
 * An isolated frame's own `localStorage` throws, and the bridge is async where
 * every reader below is synchronous. So the whole of this frame's store is
 * fetched once by [`primeBridgedStorage`] and served from here. A write updates
 * the mirror at once and tells the host after, so a read never waits.
 *
 * The host already scopes by workspace and app, so keys here are bare.
 */
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
/** What this frame wrote before the prime answered, which the older snapshot
 *  must not overwrite. Keyed `<space>:<area>:<key>`, and `<space>:<area>` for
 *  a whole area cleared. */
let writtenBeforePrime = new Set<string>();

function noteWrite(space: AppStorageSpace, area: AppStorageArea, key: string | null): void {
  if (primeSettled) return;
  writtenBeforePrime.add(key === null ? `${space}:${area}` : `${space}:${area}:${key}`);
}

/**
 * Fetch this frame's stored values before anything reads them.
 *
 * Started once at SDK load and shared by every caller, so `lucidos.storage.ready`
 * and scroll memory wait on the same answer. A failure leaves the mirror empty,
 * which reads as "nothing stored", and rejects so `ready` can say so.
 */
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

/** Whether a read can trust the mirror: the prime has answered, either way. */
export function bridgedStorageSettled(): boolean {
  return !isBridged() || primeSettled;
}

function sdkGet(key: string, area: AppStorageArea): string | null {
  return mirror.sdk[area].get(key) ?? null;
}

/** The SDK's own keys owe no answer. The host logs a refusal itself
 *  (`docs/adr/0372-app-storage-lives-in-the-shell.md`). */
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

/** Read a per-workspace localStorage value (namespaced). Null if unavailable. */
export function wsLocalGet(key: string): string | null {
  if (isBridged()) return sdkGet(key, 'local');
  try {
    return localStorage.getItem(nsKey(key));
  } catch {
    return null;
  }
}

/** The storage key the parent app mints this workspace's device id under. */
const DEVICE_ID_KEY = 'lucidos-device-id';

/**
 * The device id the parent app minted for this workspace, or null.
 *
 * Two readers, which is why it sits here rather than in either of them.
 * `preferences` scopes its read by it, and `_fetch` sends it on every request
 * so the engine can attribute what an app changed.
 */
export function wsDeviceId(): string | null {
  return wsLocalGet(DEVICE_ID_KEY);
}

/** Write a per-workspace localStorage value (namespaced). Best-effort: a
 *  storage-less realm keeps nothing. */
export function wsLocalSet(key: string, value: string): void {
  if (isBridged()) return sdkSet(key, value, 'local');
  try {
    localStorage.setItem(nsKey(key), value);
  } catch {
    /* a storage-less realm has nowhere to write */
  }
}

/** Remove a per-workspace localStorage value (namespaced). Best-effort: used by
 *  the boot script's `?style-reset` escape hatch, which must work even when the
 *  UI it is rescuing does not. */
export function wsLocalRemove(key: string): void {
  if (isBridged()) return sdkRemove(key, 'local');
  try {
    localStorage.removeItem(nsKey(key));
  } catch {
    /* a storage-less realm has nothing to clear */
  }
}

/** Read a per-workspace sessionStorage value (namespaced). Null if unavailable. */
export function wsSessionGet(key: string): string | null {
  if (isBridged()) return sdkGet(key, 'session');
  try {
    return sessionStorage.getItem(nsKey(key));
  } catch {
    return null;
  }
}

/** Write a per-workspace sessionStorage value (namespaced). Best-effort. */
export function wsSessionSet(key: string, value: string): void {
  if (isBridged()) return sdkSet(key, value, 'session');
  sessionStorage.setItem(nsKey(key), value);
}

/** Remove a per-workspace sessionStorage value (namespaced). Best-effort. */
export function wsSessionRemove(key: string): void {
  if (isBridged()) return sdkRemove(key, 'session');
  sessionStorage.removeItem(nsKey(key));
}

/**
 * One area of the app's own store, as `lucidos.storage` sees it.
 *
 * A write either throws at once or returns the host's answer. The bridged store
 * returns a promise, and a refusal there has already been undone in the mirror.
 */
export interface AppStoreBackend {
  entries(): Array<[string, string]>;
  get(key: string): string | null;
  set(key: string, value: string): Promise<void> | void;
  remove(key: string): Promise<void> | void;
  clear(): Promise<void> | void;
}

/** Which write last touched each app key, so a refusal undoes only its own. */
const lastWrite = new Map<string, number>();
let writeCount = 0;

/**
 * Apply a write to the mirror now, and tell the host.
 *
 * If the host refuses, each key goes back to what it held, unless a later
 * write has already replaced it.
 */
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

/** The app's store in an isolated frame: the primed mirror, written through. */
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

/**
 * The app's store in a standalone tab, which can reach storage itself.
 *
 * Same raw keys the host writes for a frame, so a value saved in one is there
 * in the other. A real `QuotaExceededError` from the browser throws at once.
 */
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

/** Test-only: empty the bridged mirror and forget the prime, so module state
 *  cannot leak between cases. Not part of the runtime surface. */
export function _resetBridgedStorageForTesting(): void {
  mirror = emptyMirror();
  priming = null;
  primeSettled = false;
  writtenBeforePrime = new Set();
  lastWrite.clear();
}
