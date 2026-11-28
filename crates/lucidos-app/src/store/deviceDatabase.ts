
import { peerClient, peerScopedKey } from '../../../../packages/lucidos-sdk/src/_peerScope';
import { postClientLog } from '../utils/clientLog';

export const DEVICE_DATABASE_STALL_MS = 5_000;

export const DEVICE_DATABASE_MAX_STALL_MS = 80_000;

export const BOUNDED = { bounded: true } as const;

export class DeviceDatabaseStalledError extends Error {
  constructor(readonly database: string, readonly step: 'open' | 'transaction') {
    super(`The device database "${database}" did not answer its ${step} in time`);
    this.name = 'DeviceDatabaseStalledError';
  }
}

export class DeviceDatabaseReleasedError extends Error {
  constructor(readonly database: string) {
    super(`The device database "${database}" was released when the page hid`);
    this.name = 'DeviceDatabaseReleasedError';
  }
}

export function rerunWhenPageShows(error: unknown, run: () => void): boolean {
  if (!(error instanceof DeviceDatabaseReleasedError)) return false;
  window.addEventListener('pageshow', () => run(), { once: true });
  return true;
}

interface OpenTransaction {
  database: string;
  mode: IDBTransactionMode;
  bounded: boolean;
  startedAt: number;
  release(): void;
}

const openTransactions = new Set<OpenTransaction>();
const connectionReleasers = new Set<() => void>();

/** Called on `pagehide`. iOS can freeze or replace the page right after, and
 *  a transaction it left open blocks the next page's transactions on the same
 *  store. Unbounded writes hold user work, so they finish rather than abort.
 *  The log names what was open, to confirm that holder on a real device
 *  (docs/temporary-measures.md § Device storage release log). */
export function releaseDeviceDatabases(): void {
  if (openTransactions.size > 0) {
    const now = Date.now();
    postClientLog('device-database', 'released_on_hide', { transactions: [...openTransactions].map((t) => ({
      database: t.database, mode: t.mode, bounded: t.bounded, age_ms: now - t.startedAt,
    })) });
  }
  for (const transaction of [...openTransactions]) transaction.release();
  for (const releaseConnection of connectionReleasers) releaseConnection();
}

if (typeof window !== 'undefined' && typeof window.addEventListener === 'function') {
  window.addEventListener('pagehide', releaseDeviceDatabases);
}

export function awaitQueuedWrites(writes: Promise<void>, database: string): Promise<void> {
  return new Promise((resolve, reject) => {
    const stall = setTimeout(() => reject(new DeviceDatabaseStalledError(database, 'transaction')), DEVICE_DATABASE_STALL_MS);
    void writes.then(resolve, reject).finally(() => clearTimeout(stall));
  });
}

export const UNSENT_MESSAGES_DATABASE = 'lucidos-unsent-messages';
export const PENDING_UPLOADS_DATABASE = 'lucidos-pending-uploads';

export function deviceDatabaseName(
  base: string,
  workspaceId: string | null,
  peer: string | null = peerClient(),
): string {
  return peerScopedKey(workspaceId === null ? base : `${base}:${workspaceId}`, peer);
}

export interface DeviceDatabase {
  transact<T>(
    storeName: string | string[],
    mode: IDBTransactionMode,
    body: (store: IDBObjectStore) => IDBRequest<T> | void,
    options?: { bounded?: boolean },
  ): Promise<T | undefined>;
}

export function openDeviceDatabase(name: string, createStores: (db: IDBDatabase, transaction: IDBTransaction) => void, version = 1): DeviceDatabase {
  let opening: Promise<IDBDatabase> | null = null;
  let stallMs = DEVICE_DATABASE_STALL_MS;
  const retired = new WeakSet<IDBDatabase>();

  function retire(attempt: Promise<IDBDatabase>, db: IDBDatabase): void {
    if (opening === attempt) opening = null;
    retired.add(db);
    db.close();
  }

  function onStall(fire: () => boolean): ReturnType<typeof setTimeout> {
    return setTimeout(() => {
      if (fire()) stallMs = Math.min(stallMs * 2, DEVICE_DATABASE_MAX_STALL_MS);
    }, stallMs);
  }

  connectionReleasers.add(() => {
    const attempt = opening;
    if (!attempt) return;
    opening = null;
    attempt.then((db) => retire(attempt, db), () => {});
  });

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
      req.onblocked = () => postClientLog('device-database', 'open_blocked', { name });
      req.onsuccess = () => {
        clearTimeout(stall);
        const db = req.result;
        if (stalled) {
          db.close();
          return;
        }
        db.onversionchange = () => retire(attempt, db);
        db.onclose = () => retire(attempt, db);
        resolve(db);
      };
      req.onerror = () => {
        clearTimeout(stall);
        reject(req.error);
      };
    });
    opening = attempt;
    attempt.catch(() => {
      if (opening === attempt) opening = null;
    });
    return attempt;
  }

  return {
    async transact(storeName, mode, body, options) {
      let attempt: Promise<IDBDatabase>;
      let db: IDBDatabase;
      do {
        attempt = open();
        db = await attempt;
      } while (retired.has(db));
      return new Promise((resolve, reject) => {
        const tx = db.transaction(storeName, mode);
        const req = body(tx.objectStore(typeof storeName === 'string' ? storeName : storeName[0]));
        const stalledError = new DeviceDatabaseStalledError(name, 'transaction');
        let stalled = false;
        let released = false;
        const entry: OpenTransaction = { database: name, mode, bounded: options?.bounded === true, startedAt: Date.now(), release: () => {
          if (!entry.bounded) return;
          clearTimeout(stall);
          openTransactions.delete(entry);
          released = true;
          try {
            tx.abort();
          } catch {
          }
          retire(attempt, db);
          reject(new DeviceDatabaseReleasedError(name));
        } };
        openTransactions.add(entry);
        const giveUp = () => {
          openTransactions.delete(entry);
          retire(attempt, db);
          reject(stalledError);
          return true;
        };
        let stall = options?.bounded
          ? onStall(() => {
            stalled = true;
            try {
              tx.abort();
            } catch {
              stalled = false;
              stall = onStall(giveUp);
              return false;
            }
            return giveUp();
          })
          : undefined;
        tx.oncomplete = () => {
          clearTimeout(stall);
          openTransactions.delete(entry);
          resolve(req ? req.result : undefined);
        };
        tx.onerror = () => {
          clearTimeout(stall);
          openTransactions.delete(entry);
          reject(tx.error);
        };
        tx.onabort = () => {
          clearTimeout(stall);
          openTransactions.delete(entry);
          reject(released ? new DeviceDatabaseReleasedError(name)
            : stalled ? stalledError : tx.error ?? new DOMException('The write was aborted', 'AbortError'));
        };
      });
    },
  };
}
