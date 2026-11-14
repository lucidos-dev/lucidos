/** IndexedDB on this device, for the stores that keep work a page reload must
 *  not lose: pending uploads and unsent messages. A leaf: no store imports.
 *
 *  Workspaces share one origin (ADR 0014), so every database is named per
 *  workspace. Otherwise one workspace would read, and drop, another's records. */

import { postClientLog } from '../utils/clientLog';

/** How long an open, or a bounded transaction, may first take before it counts
 *  as stalled. WebKit can leave a request unsettled after a background resume. */
export const DEVICE_DATABASE_STALL_MS = 5_000;

/** Each stall doubles the database's bound up to this cap. A hung request
 *  keeps failing, while a slow one (large retained bytes) fits on a retry. */
export const DEVICE_DATABASE_MAX_STALL_MS = 80_000;

/** The `transact` option for a step restore admission waits on: a stall
 *  there must fail, never hang. */
export const BOUNDED = { bounded: true } as const;

/** A request that did not settle within its bound. A stalled transaction was
 *  aborted, so it never commits after this rejection. */
export class DeviceDatabaseStalledError extends Error {
  constructor(readonly database: string, readonly step: 'open' | 'transaction') {
    super(`The device database "${database}" did not answer its ${step} in time`);
    this.name = 'DeviceDatabaseStalledError';
  }
}

/** `base` for a context with no workspace id (a direct engine port, legacy),
 *  else `base:<workspace id>`. */
export function deviceDatabaseName(base: string, workspaceId: string | null): string {
  return workspaceId === null ? base : `${base}:${workspaceId}`;
}

export interface DeviceDatabase {
  /** One transaction over the named stores; the callback receives the first.
   *  Resolves with the request's result once the transaction commits, and
   *  rejects if it fails or aborts. Every open is bounded. `bounded` also
   *  aborts a transaction that stalls: never pass it for a write whose loss
   *  costs user work. */
  transact<T>(
    storeName: string | string[],
    mode: IDBTransactionMode,
    body: (store: IDBObjectStore) => IDBRequest<T> | void,
    options?: { bounded?: boolean },
  ): Promise<T | undefined>;
}

/** Open lazily. Upgrades run exclusively after older connections close. */
export function openDeviceDatabase(name: string, createStores: (db: IDBDatabase, transaction: IDBTransaction) => void, version = 1): DeviceDatabase {
  let opening: Promise<IDBDatabase> | null = null;
  let stallMs = DEVICE_DATABASE_STALL_MS;

  /** `fire` answers whether the request really stalled. */
  function onStall(fire: () => boolean): ReturnType<typeof setTimeout> {
    return setTimeout(() => {
      if (fire()) stallMs = Math.min(stallMs * 2, DEVICE_DATABASE_MAX_STALL_MS);
    }, stallMs);
  }

  function open(): Promise<IDBDatabase> {
    if (opening) return opening;
    let stalled = false;
    const attempt = new Promise<IDBDatabase>((resolve, reject) => {
      const req = indexedDB.open(name, version);
      const stall = onStall(() => {
        stalled = true;
        reject(new DeviceDatabaseStalledError(name, 'open'));
        return true;
      });
      req.onupgradeneeded = () => createStores(req.result, req.transaction!);
      // Another tab holds an older version open. The stall bound turns a
      // lasting block into a failure; this names the cause in the engine log.
      req.onblocked = () => postClientLog('device-database', 'open_blocked', { name });
      req.onsuccess = () => {
        clearTimeout(stall);
        const db = req.result;
        // The caller already saw a failure, and the next call opens afresh.
        if (stalled) {
          db.close();
          return;
        }
        // Another tab is deleting or upgrading the database. The next write
        // opens a fresh connection rather than using this closed one.
        db.onversionchange = () => {
          db.close();
          if (opening === attempt) opening = null;
        };
        // WebKit can drop the connection while the page sits in the
        // background. The next write reopens rather than failing for good.
        db.onclose = () => {
          if (opening === attempt) opening = null;
        };
        resolve(db);
      };
      req.onerror = () => {
        clearTimeout(stall);
        reject(req.error);
      };
    });
    opening = attempt;
    // A failed open is retried by the next operation rather than remembered.
    attempt.catch(() => {
      if (opening === attempt) opening = null;
    });
    return attempt;
  }

  return {
    async transact(storeName, mode, body, options) {
      const db = await open();
      return new Promise((resolve, reject) => {
        const tx = db.transaction(storeName, mode);
        const req = body(tx.objectStore(typeof storeName === 'string' ? storeName : storeName[0]));
        const stalledError = new DeviceDatabaseStalledError(name, 'transaction');
        let stalled = false;
        // Abort, not just reject: a stalled write must never commit later.
        // Reject at once too, since a wedged backend may never fire `abort`.
        const stall = options?.bounded
          ? onStall(() => {
            stalled = true;
            try {
              tx.abort();
            } catch {
              // It finished as the bound ran out. Its own event settles it.
              stalled = false;
              return false;
            }
            reject(stalledError);
            return true;
          })
          : undefined;
        tx.oncomplete = () => {
          clearTimeout(stall);
          resolve(req ? req.result : undefined);
        };
        tx.onerror = () => {
          clearTimeout(stall);
          reject(tx.error);
        };
        tx.onabort = () => {
          clearTimeout(stall);
          reject(stalled ? stalledError : tx.error ?? new DOMException('The write was aborted', 'AbortError'));
        };
      });
    },
  };
}
