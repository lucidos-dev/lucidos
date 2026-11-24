
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

export interface StorageFailure {
  op: 'set' | 'remove' | 'clear';
  area: AppStorageArea;
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

function settle(failure: Omit<StorageFailure, 'message'>, outcome: Promise<void> | void): void {
  if (!outcome) return;
  outcome.catch((error: unknown) => {
    report({ ...failure, message: error instanceof Error ? error.message : String(error) });
  });
}

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
  local: new AppStore('local'),
  session: new AppStore('session'),
  get ready(): Promise<void> {
    return primeBridgedStorage();
  },
  onError(handler: (failure: StorageFailure) => void): () => void {
    failureHandlers.add(handler);
    return () => { failureHandlers.delete(handler); };
  },
  quota: APP_STORAGE_QUOTA,
  valueMax: APP_STORAGE_VALUE_MAX,
};

export function _resetStorageForTesting(): void {
  failureHandlers.clear();
  warnedEarlyRead = false;
}
