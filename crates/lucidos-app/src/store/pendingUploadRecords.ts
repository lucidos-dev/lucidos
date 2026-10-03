/** The bytes of each pending upload, kept on this device so a page reload does
 *  not lose the image. One IndexedDB database per workspace. The live entries
 *  are in `store/pendingUploads.ts`; a record here outlives its entry until the
 *  engine's draft holds the image.
 *
 *  Plan: `docs/plans/2026-10-02-pending-uploads-survive-a-reload.md`. */

import type { UploadSendIntent } from '../components/chat/prompt-input-helpers';
import { WORKSPACE_ID } from '../utils/basePath';
import { generateUuid } from '../utils/uuid';
import { showToast } from './store';

export interface PendingUploadRecord {
  localId: string;
  threadId: string;
  name: string;
  mime: string;
  /** An `ArrayBuffer`, not a `Blob`: older WebKit stored `Blob`s unreliably. */
  bytes: ArrayBuffer;
  size: number;
  createdAt: number;
  /** The page load that owns the record. See `liveOwnerIds`. */
  ownerId: string;
  /** Set once the upload landed, until the engine's draft holds this hash. */
  landedHash?: string;
}

/** A send the user pressed while the thread's images were still uploading. */
export interface QueuedUploadSendRecord {
  threadId: string;
  intent: UploadSendIntent;
  ownerId: string;
  /** What it still waits on, and the draft text it would send. A restore
   *  queues it again only if every image comes back and the draft still reads
   *  `text`. So it never sends what the user did not see. */
  localIds: string[];
  text: string;
}

/** What this page last stored for a queued upload send. */
export type QueuedUploadSendSnapshot = Pick<QueuedUploadSendRecord, 'localIds' | 'text'>;

type UploadPatch = Partial<Pick<PendingUploadRecord, 'landedHash' | 'ownerId'>>;

export interface PendingUploadBackend {
  putUpload(record: PendingUploadRecord): Promise<void>;
  patchUpload(localId: string, patch: UploadPatch): Promise<void>;
  deleteUpload(localId: string): Promise<void>;
  listUploads(): Promise<PendingUploadRecord[]>;
  putQueuedUploadSend(record: QueuedUploadSendRecord): Promise<void>;
  deleteQueuedUploadSend(threadId: string): Promise<void>;
  listQueuedUploadSends(): Promise<QueuedUploadSendRecord[]>;
}

/** The caps that bound one workspace's store. A record past a cap is not
 *  written, and an older one is dropped when the page next starts. */
export const PENDING_UPLOAD_MAX_RECORDS = 20;
export const PENDING_UPLOAD_MAX_BYTES = 100 * 1024 * 1024;
export const PENDING_UPLOAD_MAX_AGE_MS = 7 * 24 * 60 * 60 * 1000;

/** Workspaces share one origin (ADR 0014), so each gets its own database.
 *  Otherwise one workspace's startup would drop another's records as gone. */
export function pendingUploadDbName(workspaceId: string | null): string {
  return workspaceId === null ? 'lucidos-pending-uploads' : `lucidos-pending-uploads:${workspaceId}`;
}

const DB_NAME = pendingUploadDbName(WORKSPACE_ID);
const UPLOADS = 'uploads';
const QUEUED_UPLOAD_SENDS = 'queued-upload-sends';

function createIndexedDbBackend(name: string): PendingUploadBackend {
  let opening: Promise<IDBDatabase> | null = null;

  function open(): Promise<IDBDatabase> {
    if (opening) return opening;
    const attempt = new Promise<IDBDatabase>((resolve, reject) => {
      const req = indexedDB.open(name, 1);
      req.onupgradeneeded = () => {
        req.result.createObjectStore(UPLOADS, { keyPath: 'localId' });
        req.result.createObjectStore(QUEUED_UPLOAD_SENDS, { keyPath: 'threadId' });
      };
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

  async function transact<T>(
    storeName: string,
    mode: IDBTransactionMode,
    body: (store: IDBObjectStore) => IDBRequest<T> | void,
  ): Promise<T | undefined> {
    const db = await open();
    return new Promise((resolve, reject) => {
      const tx = db.transaction(storeName, mode);
      const req = body(tx.objectStore(storeName));
      tx.oncomplete = () => resolve(req ? req.result : undefined);
      tx.onerror = () => reject(tx.error);
      tx.onabort = () => reject(tx.error ?? new DOMException('The write was aborted', 'AbortError'));
    });
  }

  return {
    putUpload: async (record) => { await transact(UPLOADS, 'readwrite', (s) => s.put(record)); },
    patchUpload: async (localId, patch) => {
      await transact(UPLOADS, 'readwrite', (s) => {
        const get = s.get(localId);
        get.onsuccess = () => {
          if (get.result) s.put({ ...get.result, ...patch });
        };
      });
    },
    deleteUpload: async (localId) => { await transact(UPLOADS, 'readwrite', (s) => s.delete(localId)); },
    listUploads: async () => (await transact<PendingUploadRecord[]>(UPLOADS, 'readonly', (s) => s.getAll())) ?? [],
    putQueuedUploadSend: async (record) => { await transact(QUEUED_UPLOAD_SENDS, 'readwrite', (s) => s.put(record)); },
    deleteQueuedUploadSend: async (threadId) => { await transact(QUEUED_UPLOAD_SENDS, 'readwrite', (s) => s.delete(threadId)); },
    listQueuedUploadSends: async () => (await transact<QueuedUploadSendRecord[]>(QUEUED_UPLOAD_SENDS, 'readonly', (s) => s.getAll())) ?? [],
  };
}

/** The same contract over two maps, for tests. Vitest runs under Node, which
 *  has no IndexedDB. */
export function createMemoryPendingUploadBackend(): PendingUploadBackend {
  const uploads = new Map<string, PendingUploadRecord>();
  const queued = new Map<string, QueuedUploadSendRecord>();
  return {
    putUpload: async (record) => { uploads.set(record.localId, { ...record }); },
    patchUpload: async (localId, patch) => {
      const record = uploads.get(localId);
      if (record) uploads.set(localId, { ...record, ...patch });
    },
    deleteUpload: async (localId) => { uploads.delete(localId); },
    listUploads: async () => [...uploads.values()].map((r) => ({ ...r })),
    putQueuedUploadSend: async (record) => { queued.set(record.threadId, { ...record }); },
    deleteQueuedUploadSend: async (threadId) => { queued.delete(threadId); },
    listQueuedUploadSends: async () => [...queued.values()].map((r) => ({ ...r })),
  };
}

/** Every operation fails, so a browser without IndexedDB hears that its
 *  images will not survive a reload. */
function createUnavailableBackend(): PendingUploadBackend {
  const fail = async (): Promise<never> => {
    throw new Error('this browser has no IndexedDB');
  };
  return {
    putUpload: fail, patchUpload: fail, deleteUpload: fail, listUploads: fail,
    putQueuedUploadSend: fail, deleteQueuedUploadSend: fail, listQueuedUploadSends: fail,
  };
}

let backend: PendingUploadBackend | null = null;

function store(): PendingUploadBackend {
  backend ??= typeof indexedDB === 'undefined' ? createUnavailableBackend() : createIndexedDbBackend(DB_NAME);
  return backend;
}

/** This page load. Each record names the page that owns it. */
export const pageOwnerId = generateUuid();

/** What this page knows of a record it owns, read synchronously by the rules
 *  that decide when a record ends. */
export interface OwnedRecord {
  localId: string;
  threadId: string;
  size: number;
  landedHash?: string;
}

const owned = new Map<string, OwnedRecord>();
/** What this page last stored for each queued upload send, by thread. */
const ownedQueuedUploadSendsByThread = new Map<string, QueuedUploadSendSnapshot>();
/** Records other open tabs own. They count against the caps all the same. */
let othersUsage = { count: 0, bytes: 0 };

/** Writes run one at a time, in order, so a delete never overtakes its put. */
let writes: Promise<void> = Promise.resolve();
let warned = false;

export const PENDING_UPLOAD_OVER_CAP_MESSAGE =
  'This image will not survive a page reload: too many images are waiting to upload. It is still uploading.';

export function pendingUploadStorageFailureMessage(err: unknown): string {
  const reason = err instanceof Error ? err.message : String(err);
  return `Images waiting to upload will not survive a page reload: this browser could not save them (${reason}).`;
}

/** Said once per page. The upload itself carries on in memory either way. */
function warnOnce(message: string): void {
  if (warned) return;
  warned = true;
  showToast(message, 'warning');
}

function enqueue(write: () => Promise<void>): void {
  writes = writes.then(write).catch((err) => warnOnce(pendingUploadStorageFailureMessage(err)));
}

/** Keep a just-attached image's bytes. Returns false, and says why once, when
 *  a cap leaves it out. */
export function persistPendingUpload(args: {
  localId: string;
  threadId: string;
  name: string;
  mime: string;
  bytes: ArrayBuffer;
}): boolean {
  const size = args.bytes.byteLength;
  let count = othersUsage.count;
  let bytes = othersUsage.bytes;
  for (const r of owned.values()) {
    count += 1;
    bytes += r.size;
  }
  if (count + 1 > PENDING_UPLOAD_MAX_RECORDS || bytes + size > PENDING_UPLOAD_MAX_BYTES) {
    warnOnce(PENDING_UPLOAD_OVER_CAP_MESSAGE);
    return false;
  }
  owned.set(args.localId, { localId: args.localId, threadId: args.threadId, size });
  const record: PendingUploadRecord = { ...args, size, createdAt: Date.now(), ownerId: pageOwnerId };
  enqueue(() => store().putUpload(record));
  return true;
}

/** The upload landed as `hash`. The record now waits for the engine's draft. */
export function markPendingUploadLanded(localId: string, hash: string): void {
  const record = owned.get(localId);
  if (!record) return;
  owned.set(localId, { ...record, landedHash: hash });
  enqueue(() => store().patchUpload(localId, { landedHash: hash }));
}

/** Take over a record a previous page load left, and resume it as unlanded. */
export function adoptPendingUploadRecord(record: PendingUploadRecord): void {
  owned.set(record.localId, { localId: record.localId, threadId: record.threadId, size: record.size });
  enqueue(() => store().patchUpload(record.localId, { ownerId: pageOwnerId, landedHash: undefined }));
}

export function forgetPendingUploadRecord(localId: string): void {
  owned.delete(localId);
  enqueue(() => store().deleteUpload(localId));
}

export function ownedPendingUploadRecords(): OwnedRecord[] {
  return [...owned.values()];
}

/** Count the records other tabs own against this page's caps. */
export function notePendingUploadsOwnedElsewhere(records: readonly PendingUploadRecord[]): void {
  othersUsage = {
    count: records.length,
    bytes: records.reduce((sum, r) => sum + r.size, 0),
  };
}

export function persistQueuedUploadSend(threadId: string, intent: UploadSendIntent, snapshot: QueuedUploadSendSnapshot): void {
  ownedQueuedUploadSendsByThread.set(threadId, snapshot);
  enqueue(() => store().putQueuedUploadSend({ threadId, intent, ownerId: pageOwnerId, ...snapshot }));
}

export function adoptQueuedUploadSendRecord(record: QueuedUploadSendRecord): void {
  ownedQueuedUploadSendsByThread.set(record.threadId, { localIds: record.localIds, text: record.text });
  enqueue(() => store().putQueuedUploadSend({ ...record, ownerId: pageOwnerId }));
}

export function forgetQueuedUploadSend(threadId: string): void {
  ownedQueuedUploadSendsByThread.delete(threadId);
  enqueue(() => store().deleteQueuedUploadSend(threadId));
}

export function ownedQueuedUploadSends(): ReadonlyMap<string, QueuedUploadSendSnapshot> {
  return ownedQueuedUploadSendsByThread;
}

/** Everything stored, after every write this page has queued. A failed read
 *  rejects, so the caller can tell "nothing stored" from "could not look". */
export async function readPendingUploadStore(): Promise<{
  uploads: PendingUploadRecord[];
  queuedUploadSendRecords: QueuedUploadSendRecord[];
}> {
  await writes;
  const [uploads, queuedUploadSendRecords] = await Promise.all([store().listUploads(), store().listQueuedUploadSends()]);
  return { uploads, queuedUploadSendRecords };
}

const OWNER_LOCK_PREFIX = `${DB_NAME}:owner:`;

/** Hold a Web Lock named after this page for its whole life. Another tab
 *  reads the held locks to learn which records still have a live owner. */
export function holdPageOwnerLock(): void {
  if (typeof navigator === 'undefined' || !navigator.locks) return;
  navigator.locks.request(OWNER_LOCK_PREFIX + pageOwnerId, () => new Promise<never>(() => {})).catch((err) => {
    // Runs without user intent. Without the lock another tab may resume this
    // page's images too. The blob store is content-addressed and the draft
    // refuses a second copy of a hash, so the cost is a repeated upload.
    console.warn('pending uploads: could not hold the owner lock', err);
  });
}

/** The owners another open tab still holds, or null when this browser cannot
 *  tell. Null means every record is free to adopt. */
export async function liveOwnerIds(): Promise<ReadonlySet<string> | null> {
  if (typeof navigator === 'undefined' || !navigator.locks) return null;
  const { held = [] } = await navigator.locks.query();
  const owners = new Set<string>();
  for (const lock of held) {
    if (lock.name?.startsWith(OWNER_LOCK_PREFIX)) owners.add(lock.name.slice(OWNER_LOCK_PREFIX.length));
  }
  return owners;
}

/** `null` puts back the backend this runtime would pick on its own. */
export function _resetPendingUploadRecordsForTesting(
  next: PendingUploadBackend | null = createMemoryPendingUploadBackend(),
): void {
  backend = next;
  owned.clear();
  ownedQueuedUploadSendsByThread.clear();
  othersUsage = { count: 0, bytes: 0 };
  writes = Promise.resolve();
  warned = false;
}

/** Resolves once every queued write has run. */
export function _pendingUploadWritesSettledForTesting(): Promise<void> {
  return writes;
}
