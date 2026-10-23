/**
 * `lucidos.storage`: per-app, per-device storage an isolated app frame can use.
 *
 * A frame's own `localStorage` throws, because its origin is opaque. These two
 * stores answer the same `Storage` calls from a mirror the host primes, and
 * write through to the shell's own storage. Await `ready` before the first
 * read. Contract: `system-knowhow/js-sdk.md` § lucidos.storage. Why it works
 * this way: `docs/adr/0372-app-storage-lives-in-the-shell.md`.
 */

import { getBaseUrl } from './_fetch';
import { isBridged } from './_bridge';
import {
  bridgedAppBackend,
  bridgedStorageSettled,
  directAppBackend,
  primeBridgedStorage,
  type AppStoreBackend,
} from './_storage';
import {
  APP_STORAGE_QUOTA,
  APP_STORAGE_VALUE_MAX,
  appStorageRefusal,
  type AppStorageArea,
} from './appStorage';
import { parseAppId } from './scroll';

/** A write the host refused after `setItem`, `removeItem` or `clear` returned. */
export interface StorageFailure {
  op: 'set' | 'remove' | 'clear';
  area: AppStorageArea;
  /** The key written, or null for `clear`. */
  key: string | null;
  message: string;
}

const failureHandlers = new Set<(failure: StorageFailure) => void>();

function report(failure: StorageFailure): void {
  if (failureHandlers.size === 0) {
    console.error(`[lucidos-sdk] lucidos.storage could not ${failure.op}:`, failure.message);
    return;
  }
  for (const handler of failureHandlers) handler(failure);
}

let warnedEarlyRead = false;

/** A read before the prime answered sees what a first visit sees. Said once,
 *  because it is the bug that makes an app forget its state on reload. */
function warnIfEarly(area: AppStorageArea, key: string | null): void {
  if (warnedEarlyRead || bridgedStorageSettled()) return;
  warnedEarlyRead = true;
  const what = key === null ? 'its size' : `"${key}"`;
  console.warn(
    `[lucidos-sdk] lucidos.storage.${area} read ${what} before lucidos.storage.ready resolved,`
    + ' so it saw nothing stored. Await lucidos.storage.ready first.',
  );
}

function backendFor(area: AppStorageArea): AppStoreBackend | null {
  if (isBridged()) return bridgedAppBackend(area);
  const appId = parseAppId(window.location.pathname, getBaseUrl());
  return appId ? directAppBackend(area, appId) : null;
}

function notAnApp(): Error {
  return new Error('lucidos.storage needs an app: this page is not served from /app/<id>/');
}

/** Turn a write's outcome into what a caller sees. A synchronous refusal
 *  throws; a later one from the host goes to the `onError` handlers. */
function settle(failure: Omit<StorageFailure, 'message'>, outcome: Promise<void> | void): void {
  if (!outcome) return;
  outcome.catch((error: unknown) => {
    report({ ...failure, message: error instanceof Error ? error.message : String(error) });
  });
}

/** One area of the app's store, with the `Storage` interface. */
export class AppStore {
  constructor(private readonly area: AppStorageArea) {}

  get length(): number {
    warnIfEarly(this.area, null);
    return backendFor(this.area)?.entries().length ?? 0;
  }

  key(index: number): string | null {
    warnIfEarly(this.area, null);
    return backendFor(this.area)?.entries()[index]?.[0] ?? null;
  }

  getItem(key: string): string | null {
    const k = String(key);
    warnIfEarly(this.area, k);
    return backendFor(this.area)?.get(k) ?? null;
  }

  setItem(key: string, value: string): void {
    const k = String(key);
    const v = String(value);
    const backend = backendFor(this.area);
    if (!backend) throw notAnApp();
    const refusal = appStorageRefusal(backend.entries(), k, v);
    if (refusal) throw new DOMException(refusal, 'QuotaExceededError');
    settle({ op: 'set', area: this.area, key: k }, backend.set(k, v));
  }

  removeItem(key: string): void {
    const k = String(key);
    const backend = backendFor(this.area);
    if (!backend) throw notAnApp();
    settle({ op: 'remove', area: this.area, key: k }, backend.remove(k));
  }

  clear(): void {
    const backend = backendFor(this.area);
    if (!backend) throw notAnApp();
    settle({ op: 'clear', area: this.area, key: null }, backend.clear());
  }
}

export const storage = {
  /** Kept across reloads and app switches, on this device. */
  local: new AppStore('local'),
  /** Kept until the Lucidos tab closes, on this device. */
  session: new AppStore('session'),
  /** Resolves once this frame's stored values are readable. Rejects if the
   *  host could not hand them over, in which case reads see nothing stored. */
  get ready(): Promise<void> {
    return primeBridgedStorage();
  },
  /** Hear about a write the host refused after the call returned, such as a
   *  full browser store. The value is already rolled back. Returns the
   *  unsubscribe. */
  onError(handler: (failure: StorageFailure) => void): () => void {
    failureHandlers.add(handler);
    return () => { failureHandlers.delete(handler); };
  },
  /** How much one app may keep in one area, in characters of key plus value. */
  quota: APP_STORAGE_QUOTA,
  /** The largest single value, in characters. */
  valueMax: APP_STORAGE_VALUE_MAX,
};

/** Test-only: forget handlers and the one-shot warning. */
export function _resetStorageForTesting(): void {
  failureHandlers.clear();
  warnedEarlyRead = false;
}
