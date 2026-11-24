import { sameStoredUpload, sameUploadBytes, uploadFileIdentity, uploadSourceVersion,
  type StoredUploadSource } from './pendingUploadSource';
import { readUploadPayload, retainUploadBytes, UPLOAD_BYTES } from './uploadByteStore';
import { queuedRestoreSourceId } from '../utils/restoreAdmission';
import { BOUNDED, openDeviceDatabase } from './deviceDatabase';
import type { PendingUploadBackend, PendingUploadRecord, QueuedUploadSendRecord } from './pendingUploadRecords';

export const PENDING_UPLOADS_STORE = 'uploads';
const UPLOAD_HISTORY = 'upload-source-history';

function retainUpload(history: IDBObjectStore, record: PendingUploadRecord): void {
  retainUploadBytes(history.transaction.objectStore(UPLOAD_BYTES), record, false, (source) => {
    const key = uploadSourceVersion(source);
    const get = history.get(key);
    get.onsuccess = () => {
      if (get.result === undefined) history.add(source, key);
      else if (!sameStoredUpload(get.result, source)) history.transaction.abort();
    };
  });
}

const QUEUED_UPLOAD_SENDS = 'queued-upload-sends';

function queuedSlot(record: QueuedUploadSendRecord): string {
  return JSON.stringify([record.threadId, record.ownerId, record.sourceGeneration ?? null]);
}
function queuedVersion(record: QueuedUploadSendRecord): string {
  return JSON.stringify([queuedSlot(record), queuedRestoreSourceId(record)]);
}
const QUEUED_HISTORY = 'queued-source-history';

export function createIndexedDbBackend(name: string): PendingUploadBackend {
  const { transact } = openDeviceDatabase(name, (db, tx) => {
    if (!db.objectStoreNames.contains(PENDING_UPLOADS_STORE)) db.createObjectStore(PENDING_UPLOADS_STORE, { keyPath: 'localId' });
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
    const bytes = db.createObjectStore(UPLOAD_BYTES);
    const hadHistory = db.objectStoreNames.contains(UPLOAD_HISTORY);
    const history = hadHistory ? tx.objectStore(UPLOAD_HISTORY) : db.createObjectStore(UPLOAD_HISTORY);
    const legacy = (hadHistory ? history : tx.objectStore(PENDING_UPLOADS_STORE)).openCursor();
    legacy.onsuccess = () => {
      const cursor = legacy.result;
      if (!cursor) {
        if (hadHistory) {
          const active = tx.objectStore(PENDING_UPLOADS_STORE).openCursor();
          active.onsuccess = () => {
            const entry = active.result;
            if (!entry) return;
            entry.update({ ...entry.value, migratedBytes: true });
            entry.continue();
          };
        }
        return;
      }
      retainUploadBytes(bytes, cursor.value, hadHistory, (source) => {
        history.put(source, hadHistory ? cursor.primaryKey : uploadSourceVersion(source));
        cursor.continue();
      });
    };
  }, 4);

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
      await transact([PENDING_UPLOADS_STORE, UPLOAD_HISTORY, UPLOAD_BYTES], 'readwrite', (s) => {
        retainUpload(s.transaction.objectStore(UPLOAD_HISTORY), record);
        s.put(record);
      });
    },
    patchUpload: async (localId, patch) => {
      await transact([PENDING_UPLOADS_STORE, UPLOAD_HISTORY, UPLOAD_BYTES], 'readwrite', (s) => {
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
      await transact([PENDING_UPLOADS_STORE, UPLOAD_HISTORY, UPLOAD_BYTES], 'readwrite', (s) => {
        s.delete(localId);
        const bytes = s.transaction.objectStore(UPLOAD_BYTES);
        const history = s.transaction.objectStore(UPLOAD_HISTORY).openCursor();
        history.onsuccess = () => {
          const cursor = history.result;
          if (!cursor) return;
          const source = cursor.value as StoredUploadSource;
          if (source.localId === localId) {
            cursor.delete();
            bytes.delete(source.byteKey);
          }
          cursor.continue();
        };
      });
    },
    listUploadHistory: async () => (await transact<StoredUploadSource[]>(UPLOAD_HISTORY, 'readonly', (s) => s.getAll(), BOUNDED)) ?? [],
    readUploadBytes: async (source) => {
      let payload: ReturnType<typeof readUploadPayload> | undefined;
      await transact([UPLOAD_HISTORY, UPLOAD_BYTES], 'readonly', (s) => {
        const version = s.get(uploadSourceVersion(source));
        version.onsuccess = () => {
          if (version.result && sameStoredUpload(version.result, source)) {
            payload = readUploadPayload(s.transaction.objectStore(UPLOAD_BYTES), source.byteKey);
          }
        };
      }, BOUNDED);
      if (!payload) return undefined;
      if (!payload.result) throw new Error('Archived upload bytes are missing');
      return payload.result.bytes;
    },
    listUploads: async () => (await transact<PendingUploadRecord[]>(PENDING_UPLOADS_STORE, 'readonly', (s) => s.getAll(), BOUNDED)) ?? [],
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
      (s) => s.getAll(IDBKeyRange.bound('active:', 'active;')), BOUNDED)) ?? [],
    listQueuedUploadHistory: async () => (await transact<QueuedUploadSendRecord[]>(QUEUED_HISTORY, 'readonly',
      (s) => s.getAll(sourceRange()), BOUNDED)) ?? [],
  };
}

export function createMemoryPendingUploadBackend(): PendingUploadBackend {
  const uploads = new Map<string, PendingUploadRecord>();
  const uploadHistory = new Map<string, StoredUploadSource>();
  const uploadBytes = new Map<string, ArrayBuffer>();
  function retain(record: PendingUploadRecord): void {
    const { bytes, ...metadata } = record;
    const identity = uploadFileIdentity(record);
    const previous = uploadBytes.get(identity);
    if (previous && !sameUploadBytes(previous, bytes)) throw new Error('Upload source bytes changed');
    if (!previous) uploadBytes.set(identity, bytes.slice(0));
    uploadHistory.set(uploadSourceVersion(record), structuredClone({ ...metadata, byteKey: [identity, 0] }));
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
      for (const [key, record] of uploadHistory) {
        if (record.localId !== localId) continue;
        uploadHistory.delete(key);
        uploadBytes.delete(record.byteKey[0]);
      }
    },
    listUploadHistory: async () => [...uploadHistory.values()].map((r) => structuredClone(r)),
    readUploadBytes: async (source) => {
      const current = uploadHistory.get(uploadSourceVersion(source));
      if (!current || !sameStoredUpload(current, source)) return undefined;
      const bytes = uploadBytes.get(source.byteKey[0]);
      if (!bytes) throw new Error('Archived upload bytes are missing');
      return bytes.slice(0);
    },
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

