/** Device-local upload storage. No application state or browser startup effects. */
import { uploadSourceVersion } from './pendingUploadSource';
import { queuedRestoreSourceId } from '../utils/restoreAdmission';
import { openDeviceDatabase } from './deviceDatabase';
import type { PendingUploadBackend, PendingUploadRecord, QueuedUploadSendRecord } from './pendingUploadRecords';

const UPLOADS = 'uploads';
const UPLOAD_HISTORY = 'upload-source-history';

// Stable source metadata plus exact byte equality make a repeated write idempotent.
// Reusing a source identity for different bytes is refused, never overwritten.
function sameBytes(a: ArrayBuffer, b: ArrayBuffer): boolean {
  const left = new Uint8Array(a), right = new Uint8Array(b);
  return left.length === right.length && left.every((v, i) => v === right[i]);
}
function retainUpload(store: IDBObjectStore, record: PendingUploadRecord): void {
  const key = uploadSourceVersion(record);
  const get = store.get(key);
  get.onsuccess = () => {
    if (!get.result) store.add(record, key);
    else if (!sameBytes(get.result.bytes, record.bytes)) store.transaction.abort();
  };
}

const QUEUED_UPLOAD_SENDS = 'queued-upload-sends';

/** Active slots are scoped to a page and generation. History holds the intent
 * a restore could strand, so a late write from an obsolete page is journaled at
 * the next hello. A settled or superseded revision leaves history: a delivered
 * send is in the workspace, so the recovery snapshot covers a later restore. */
function queuedSlot(record: QueuedUploadSendRecord): string {
  return JSON.stringify([record.threadId, record.ownerId, record.sourceGeneration ?? null]);
}
function queuedVersion(record: QueuedUploadSendRecord): string {
  return JSON.stringify([queuedSlot(record), queuedRestoreSourceId(record)]);
}
const QUEUED_HISTORY = 'queued-source-history';

export function createIndexedDbBackend(name: string): PendingUploadBackend {
  // Upgrades close both v1 and v2 pages. Their explicit old-version reopen
  // fails, so they cannot delete bytes outside this retention contract.
  const { transact } = openDeviceDatabase(name, (db, tx) => {
    if (!db.objectStoreNames.contains(UPLOADS)) db.createObjectStore(UPLOADS, { keyPath: 'localId' });
    if (!db.objectStoreNames.contains(QUEUED_UPLOAD_SENDS)) db.createObjectStore(QUEUED_UPLOAD_SENDS, { keyPath: 'threadId' });
    if (!db.objectStoreNames.contains(QUEUED_HISTORY)) {
      const history = db.createObjectStore(QUEUED_HISTORY);
      const legacy = tx.objectStore(QUEUED_UPLOAD_SENDS).openCursor();
      legacy.onsuccess = () => {
        const cursor = legacy.result;
        if (!cursor) return;
        const record = cursor.value as QueuedUploadSendRecord;
        history.put(record, `active:${queuedSlot(record)}`);
        history.put(record, `source:${queuedVersion(record)}`);
        cursor.continue();
      };
    }
    const history = db.createObjectStore(UPLOAD_HISTORY);
    const uploads = tx.objectStore(UPLOADS).openCursor();
    uploads.onsuccess = () => {
      const cursor = uploads.result;
      if (!cursor) return;
      retainUpload(history, cursor.value);
      cursor.continue();
    };
  }, 3);

  /** Delete every history entry `matches` selects, in the open transaction. */
  function dropHistory<T>(store: IDBObjectStore, range: IDBKeyRange | null, matches: (value: T) => boolean): void {
    const cursor = store.openCursor(range);
    cursor.onsuccess = () => {
      const entry = cursor.result;
      if (!entry) return;
      if (matches(entry.value as T)) entry.delete();
      entry.continue();
    };
  }
  const sourceRange = () => IDBKeyRange.bound('source:', 'source;');

  return {
    putUpload: async (record) => {
      await transact([UPLOADS, UPLOAD_HISTORY], 'readwrite', (s) => {
        retainUpload(s.transaction.objectStore(UPLOAD_HISTORY), record);
        s.put(record);
      });
    },
    patchUpload: async (localId, patch) => {
      await transact([UPLOADS, UPLOAD_HISTORY], 'readwrite', (s) => {
        const get = s.get(localId);
        get.onsuccess = () => {
          if (get.result) {
            const record = { ...get.result, ...patch };
            retainUpload(s.transaction.objectStore(UPLOAD_HISTORY), record);
            s.put(record);
          }
        };
      });
    },
    deleteUpload: async (localId) => {
      await transact([UPLOADS, UPLOAD_HISTORY], 'readwrite', (s) => {
        s.delete(localId);
        dropHistory<PendingUploadRecord>(s.transaction.objectStore(UPLOAD_HISTORY), null, (r) => r.localId === localId);
      });
    },
    listUploadHistory: async () => (await transact<PendingUploadRecord[]>(UPLOAD_HISTORY, 'readonly', (s) => s.getAll())) ?? [],
    listUploads: async () => (await transact<PendingUploadRecord[]>(UPLOADS, 'readonly', (s) => s.getAll())) ?? [],
    putQueuedUploadSend: async (record) => {
      await transact(QUEUED_HISTORY, 'readwrite', (s) => {
        const active = s.get(`active:${queuedSlot(record)}`);
        active.onsuccess = () => {
          const previous = active.result as QueuedUploadSendRecord | undefined;
          if (previous && queuedVersion(previous) !== queuedVersion(record)) s.delete(`source:${queuedVersion(previous)}`);
          s.put(record, `source:${queuedVersion(record)}`);
          s.put(record, `active:${queuedSlot(record)}`);
        };
      });
    },
    deleteQueuedUploadSend: async (record) => {
      await transact(QUEUED_HISTORY, 'readwrite', (s) => {
        const key = `active:${queuedSlot(record)}`;
        const get = s.get(key);
        get.onsuccess = () => {
          if (!get.result || queuedVersion(get.result) !== queuedVersion(record)) return;
          s.delete(key);
          dropHistory<QueuedUploadSendRecord>(s, sourceRange(), (r) => queuedSlot(r) === queuedSlot(record));
        };
      });
    },
    listQueuedUploadSends: async () => (await transact<QueuedUploadSendRecord[]>(QUEUED_HISTORY, 'readonly',
      (s) => s.getAll(IDBKeyRange.bound('active:', 'active;')))) ?? [],
    listQueuedUploadHistory: async () => (await transact<QueuedUploadSendRecord[]>(QUEUED_HISTORY, 'readonly',
      (s) => s.getAll(sourceRange()))) ?? [],
  };
}

/** In-memory implementation of the same isolation contract, for tests. */
export function createMemoryPendingUploadBackend(): PendingUploadBackend {
  const uploads = new Map<string, PendingUploadRecord>();
  const uploadHistory = new Map<string, PendingUploadRecord>();
  function retain(record: PendingUploadRecord): void {
    const key = uploadSourceVersion(record);
    const previous = uploadHistory.get(key);
    if (previous && !sameBytes(previous.bytes, record.bytes)) throw new Error('Upload source bytes changed');
    uploadHistory.set(key, structuredClone(record));
  }
  const queued = new Map<string, QueuedUploadSendRecord>();
  const history = new Map<string, QueuedUploadSendRecord>();
  return {
    putUpload: async (record) => { retain(record); uploads.set(record.localId, structuredClone(record)); },
    patchUpload: async (localId, patch) => {
      const record = uploads.get(localId);
      if (record) { const next = { ...record, ...patch }; retain(next); uploads.set(localId, next); }
    },
    deleteUpload: async (localId) => {
      uploads.delete(localId);
      for (const [key, record] of uploadHistory) if (record.localId === localId) uploadHistory.delete(key);
    },
    listUploadHistory: async () => [...uploadHistory.values()].map((r) => structuredClone(r)),
    listUploads: async () => [...uploads.values()].map((r) => ({ ...r })),
    putQueuedUploadSend: async (record) => {
      const previous = queued.get(queuedSlot(record));
      if (previous && queuedVersion(previous) !== queuedVersion(record)) history.delete(queuedVersion(previous));
      queued.set(queuedSlot(record), structuredClone(record));
      history.set(queuedVersion(record), structuredClone(record));
    },
    deleteQueuedUploadSend: async (record) => {
      const current = queued.get(queuedSlot(record));
      if (!current || queuedVersion(current) !== queuedVersion(record)) return;
      queued.delete(queuedSlot(record));
      for (const [key, entry] of history) if (queuedSlot(entry) === queuedSlot(record)) history.delete(key);
    },
    listQueuedUploadHistory: async () => [...history.values()].map((r) => structuredClone(r)),
    listQueuedUploadSends: async () => [...queued.values()].map((r) => ({ ...r })),
  };
}

