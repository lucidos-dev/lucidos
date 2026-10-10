/** IndexedDB on this device, for the stores that keep work a page reload must
 *  not lose: pending uploads and unsent messages. A leaf: no store imports.
 *
 *  Workspaces share one origin (ADR 0014), so every database is named per
 *  workspace. Otherwise one workspace would read, and drop, another's records. */

/** `base` for a context with no workspace id (a direct engine port, legacy),
 *  else `base:<workspace id>`. */
export function deviceDatabaseName(base: string, workspaceId: string | null): string {
  return workspaceId === null ? base : `${base}:${workspaceId}`;
}

export interface DeviceDatabase {
  /** One transaction on one object store. Resolves with the request's result
   *  once the transaction commits, and rejects if it fails or aborts. */
  transact<T>(
    storeName: string,
    mode: IDBTransactionMode,
    body: (store: IDBObjectStore) => IDBRequest<T> | void,
  ): Promise<T | undefined>;
}

/** Open lazily, at version 1, creating the object stores on first open. */
export function openDeviceDatabase(name: string, createStores: (db: IDBDatabase) => void): DeviceDatabase {
  let opening: Promise<IDBDatabase> | null = null;

  function open(): Promise<IDBDatabase> {
    if (opening) return opening;
    const attempt = new Promise<IDBDatabase>((resolve, reject) => {
      const req = indexedDB.open(name, 1);
      req.onupgradeneeded = () => createStores(req.result);
      req.onsuccess = () => {
        const db = req.result;
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
      req.onerror = () => reject(req.error);
    });
    opening = attempt;
    // A failed open is retried by the next operation rather than remembered.
    attempt.catch(() => {
      if (opening === attempt) opening = null;
    });
    return attempt;
  }

  return {
    async transact(storeName, mode, body) {
      const db = await open();
      return new Promise((resolve, reject) => {
        const tx = db.transaction(storeName, mode);
        const req = body(tx.objectStore(storeName));
        tx.oncomplete = () => resolve(req ? req.result : undefined);
        tx.onerror = () => reject(tx.error);
        tx.onabort = () => reject(tx.error ?? new DOMException('The write was aborted', 'AbortError'));
      });
    },
  };
}
