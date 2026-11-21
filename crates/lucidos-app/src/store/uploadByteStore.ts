/** Each archive owns its bytes. Variant keys are meaningful only in that database. */
import type { PendingUploadRecord } from './pendingUploadRecords';
import { sameUploadBytes, uploadFileIdentity, type StoredUploadSource, type UploadByteKey } from './pendingUploadSource';

export const UPLOAD_BYTES = 'upload-bytes';

interface UploadPayload {
  bytes: ArrayBuffer;
  migratedBytes?: true;
}

/** Keep the transaction active through the lookup, comparison and insertion.
 * Process callers' records in sequence so an earlier insert is visible. */
export function retainUploadBytes(store: IDBObjectStore, record: PendingUploadRecord,
  migrating: boolean, retained: (source: StoredUploadSource) => void): void {
  const { bytes, ...metadata } = record;
  const identity = uploadFileIdentity(record);
  function inspect(variant: number): void {
    const key: UploadByteKey = [identity, variant];
    const get = store.get(key);
    get.onsuccess = () => {
      const stored = get.result as UploadPayload | undefined;
      if (stored) {
        if (!sameUploadBytes(stored.bytes, bytes)) {
          inspect(variant + 1);
          return;
        }
        retained({ ...metadata, migratedBytes: stored.migratedBytes, byteKey: key });
        return;
      }
      if (variant !== 0 && !migrating && !record.migratedBytes) {
        store.transaction.abort();
        return;
      }
      const migratedBytes = migrating || record.migratedBytes ? true : undefined;
      const add = store.add({ bytes, migratedBytes } satisfies UploadPayload, key);
      add.onsuccess = () => retained({ ...metadata, migratedBytes, byteKey: key });
    };
  }
  inspect(0);
}

export function readUploadPayload(store: IDBObjectStore, key: UploadByteKey): IDBRequest<UploadPayload | undefined> {
  return store.get(key);
}
