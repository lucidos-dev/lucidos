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
 *  aborted, or had begun committing and then never finished. A stalled wait for
 *  queued writes leaves them running. Bounded steps are idempotent, so a caller
 *  may retry either way. The next operation opens a fresh connection, since a
 *  stall can wedge the one it ran on. */
export class DeviceDatabaseStalledError extends Error {
  constructor(readonly database: string, readonly step: 'open' | 'transaction') {
    super(`The device database "${database}" did not answer its ${step} in time`);
    this.name = 'DeviceDatabaseStalledError';
  }
}

/** A bounded transaction this page aborted because the page hid. Like a
 *  stall, the step is idempotent and its caller retries on a later attempt. */
export class DeviceDatabaseReleasedError extends Error {
  constructor(readonly database: string) {
    super(`The device database "${database}" was released when the page hid`);
    this.name = 'DeviceDatabaseReleasedError';
  }
}

/** For a one-shot reader: when the page hid mid-read, read again once the
 *  page shows. True when it will, so the caller has nothing to report. */
export function rerunWhenPageShows(error: unknown, run: () => void): boolean {
  if (!(error instanceof DeviceDatabaseReleasedError)) return false;
  window.addEventListener('pageshow', () => run(), { once: true });
  return true;
}

/** A transaction this page started that has not settled yet. */
interface OpenTransaction {
  database: string;
  mode: IDBTransactionMode;
  bounded: boolean;
  startedAt: number;
  /** Aborts a bounded transaction and fails its caller at once. */
  release(): void;
}

const openTransactions = new Set<OpenTransaction>();
/** One per open database: retires its current connection. */
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

/** Waits for a store's queued writes as long as a bounded transaction first
 *  may. A read behind them must not hang restore admission. The writes keep
 *  running past the bound, since dropping one would lose user work. */
export function awaitQueuedWrites(writes: Promise<void>, database: string): Promise<void> {
  return new Promise((resolve, reject) => {
    const stall = setTimeout(() => reject(new DeviceDatabaseStalledError(database, 'transaction')), DEVICE_DATABASE_STALL_MS);
    void writes.then(resolve, reject).finally(() => clearTimeout(stall));
  });
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
  /** Closed connections. An operation that awaited one before it closed has
   *  not begun its transaction there, so it opens afresh. */
  const retired = new WeakSet<IDBDatabase>();

  /** Only `attempt`'s own connection is dropped: a late event from an old one
   *  must not discard its replacement. */
  function retire(attempt: Promise<IDBDatabase>, db: IDBDatabase): void {
    if (opening === attempt) opening = null;
    retired.add(db);
    // Running transactions still finish, so another caller's write commits.
    db.close();
  }

  /** `fire` answers whether the request really stalled. */
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
        db.onversionchange = () => retire(attempt, db);
        // WebKit can drop the connection while the page sits in the
        // background. The next write reopens rather than failing for good.
        db.onclose = () => retire(attempt, db);
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
            // It began committing, so nothing is left to hold.
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
        // Abort, not just reject: a stalled write must never commit later.
        // Reject at once too, since a wedged backend may never fire `abort`.
        let stall = options?.bounded
          ? onStall(() => {
            stalled = true;
            try {
              tx.abort();
            } catch {
              // It began committing as the bound ran out, so its own event
              // settles it. A wedged backend never sends one: fail one bound on.
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
