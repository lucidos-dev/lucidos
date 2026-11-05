/** The bytes of each pending upload, kept on this device so a page reload does
 *  not lose the image. One IndexedDB database per workspace. The live entries
 *  are in `store/pendingUploads.ts`; a record here outlives its entry while
 *  attached or referenced by an unresolved send.
 *
 *  Plan: `docs/plans/2026-10-02-pending-uploads-survive-a-reload.md`. */

import { sourceGeneration, type RestoreSourceOrigin } from '../utils/restoreSourceGeneration';
import type { UploadSendIntent } from './uploadSendIntents';
import { signal } from '@preact/signals';
import { WORKSPACE_ID } from '../utils/basePath';
import { deviceDatabaseName } from './deviceDatabase';
import { createIndexedDbBackend } from './pendingUploadBackend';
export { createMemoryPendingUploadBackend } from './pendingUploadBackend';
import { createMemoryPendingUploadBackend } from './pendingUploadBackend';
import { pageOwnerId } from './pageOwner';
import { showToast } from './store';

export interface PendingUploadRecord extends RestoreSourceOrigin {
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
  /** Content hash once landed, while the retained bytes are still needed. */
  landedHash?: string;
}

/** A send the user pressed while the thread's images were still uploading. */
export interface QueuedUploadSendRecord extends RestoreSourceOrigin {
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
  listUploadHistory(): Promise<PendingUploadRecord[]>;
  listUploads(): Promise<PendingUploadRecord[]>;
  putQueuedUploadSend(record: QueuedUploadSendRecord): Promise<void>;
  deleteQueuedUploadSend(record: QueuedUploadSendRecord): Promise<void>;
  listQueuedUploadHistory(): Promise<QueuedUploadSendRecord[]>;
  listQueuedUploadSends(): Promise<QueuedUploadSendRecord[]>;
}

/** The caps that bound one workspace's store. A record past a cap is not
 *  written, and an older one is dropped when the page next starts. */
export const PENDING_UPLOAD_MAX_RECORDS = 20;
export const PENDING_UPLOAD_MAX_BYTES = 100 * 1024 * 1024;
export const PENDING_UPLOAD_MAX_AGE_MS = 7 * 24 * 60 * 60 * 1000;

/** One database per workspace (`deviceDatabaseName`). */
export function pendingUploadDbName(workspaceId: string | null): string {
  return deviceDatabaseName('lucidos-pending-uploads', workspaceId);
}

const DB_NAME = pendingUploadDbName(WORKSPACE_ID);
/** Every operation fails, so a browser without IndexedDB hears that its
 *  images will not survive a reload. */
function createUnavailableBackend(): PendingUploadBackend {
  const fail = async (): Promise<never> => {
    throw new Error('this browser has no IndexedDB');
  };
  return {
    putUpload: fail, patchUpload: fail, deleteUpload: fail, listUploads: fail, listUploadHistory: fail,
    putQueuedUploadSend: fail, deleteQueuedUploadSend: fail, listQueuedUploadSends: fail, listQueuedUploadHistory: fail,
  };
}

let backend: PendingUploadBackend | null = null;

function store(): PendingUploadBackend {
  backend ??= typeof indexedDB === 'undefined' ? createUnavailableBackend() : createIndexedDbBackend(DB_NAME);
  return backend;
}

/** What this page knows of a record it owns, read synchronously by the rules
 *  that decide when a record ends. */
export interface OwnedRecord {
  localId: string;
  threadId: string;
  size: number;
  landedHash?: string;
}

const owned = new Map<string, OwnedRecord>();
// Live bytes outlast pending entries even when IndexedDB or its cap refused
// them. Explicit reference cleanup below is their lifetime, not upload landing.
const live = new Map<string, PendingUploadRecord>();

/** A send takes ownership before its draft/card clears and releases only
 * after its accepted, refused or unsent destination is installed. Overlapping
 * sends may reference the same content hash. */
export const sendingImageHashes = signal<readonly string[]>([]);
const sending = new Map<symbol, readonly string[]>();
export function retainSendingImageHashes(hashes: readonly string[] = []): () => void {
  const key = Symbol();
  sending.set(key, [...hashes]);
  sendingImageHashes.value = [...sending.values()].flat();
  return () => {
    if (!sending.delete(key)) return;
    sendingImageHashes.value = [...sending.values()].flat();
  };
}

export function livePendingUploadRecords(): PendingUploadRecord[] {
  return [...live.values()];
}
/** What this page last stored for each queued upload send, by thread. */
const ownedQueuedUploadSendsByThread = new Map<string, QueuedUploadSendRecord>();
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
  const record: PendingUploadRecord = { ...args, size, createdAt: Date.now(), ownerId: pageOwnerId, sourceGeneration: sourceGeneration() };
  live.set(args.localId, record);
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
  enqueue(() => store().putUpload(record));
  return true;
}

/** The upload landed as `hash`. Retain bytes while the image is attached. */
export function markPendingUploadLanded(localId: string, hash: string): void {
  const inMemory = live.get(localId);
  if (inMemory) live.set(localId, { ...inMemory, landedHash: hash });
  const record = owned.get(localId);
  if (!record) return;
  owned.set(localId, { ...record, landedHash: hash });
  enqueue(() => store().patchUpload(localId, { landedHash: hash }));
}

/** Take over a record, retaining its landed identity only when already attached. */
export function adoptPendingUploadRecord(record: PendingUploadRecord, retainLanded = false): void {
  const landedHash = retainLanded ? record.landedHash : undefined;
  live.set(record.localId, { ...record, ownerId: pageOwnerId, landedHash });
  owned.set(record.localId, { localId: record.localId, threadId: record.threadId, size: record.size, landedHash });
  enqueue(() => store().patchUpload(record.localId, { ownerId: pageOwnerId, landedHash }));
}

export function forgetPendingUploadRecord(localId: string): void {
  live.delete(localId);
  owned.delete(localId);
  enqueue(() => store().deleteUpload(localId));
}

export function ownedPendingUploadRecords(): OwnedRecord[] {
  return [...new Map<string, OwnedRecord>([...owned.entries(), ...live.entries()]).values()];
}

/** Count the records other tabs own against this page's caps. */
export function notePendingUploadsOwnedElsewhere(records: readonly PendingUploadRecord[]): void {
  othersUsage = {
    count: records.length,
    bytes: records.reduce((sum, r) => sum + r.size, 0),
  };
}

export function persistQueuedUploadSend(threadId: string, intent: UploadSendIntent, snapshot: QueuedUploadSendSnapshot): void {
  const record = { threadId, intent, ownerId: pageOwnerId, ...snapshot, sourceGeneration: sourceGeneration() };
  ownedQueuedUploadSendsByThread.set(threadId, record);
  enqueue(() => store().putQueuedUploadSend(record));
}

export function adoptQueuedUploadSendRecord(record: QueuedUploadSendRecord): void {
  const adopted = { ...record, ownerId: pageOwnerId };
  ownedQueuedUploadSendsByThread.set(record.threadId, adopted);
  enqueue(async () => {
    await store().putQueuedUploadSend(adopted);
    if (record.ownerId !== pageOwnerId) await store().deleteQueuedUploadSend(record);
  });
}

export function forgetQueuedUploadSend(threadId: string, source = ownedQueuedUploadSendsByThread.get(threadId)): void {
  if (!source) return;
  if (ownedQueuedUploadSendsByThread.get(threadId) === source) ownedQueuedUploadSendsByThread.delete(threadId);
  enqueue(() => store().deleteQueuedUploadSend(source));
}

export function ownedQueuedUploadSends(): ReadonlyMap<string, QueuedUploadSendSnapshot> {
  return ownedQueuedUploadSendsByThread;
}

/** Everything stored, after every write this page has queued. A failed read
 *  rejects, so the caller can tell "nothing stored" from "could not look". */
export async function readPendingUploadStore(includeHistory = false): Promise<{
  uploads: PendingUploadRecord[];
  queuedUploadSendRecords: QueuedUploadSendRecord[];
}> {
  await writes;
  const [uploads, queuedUploadSendRecords] = await Promise.all([(includeHistory ? store().listUploadHistory() : store().listUploads()), (includeHistory ? store().listQueuedUploadHistory() : store().listQueuedUploadSends())]);
  return { uploads, queuedUploadSendRecords };
}

/** `null` puts back the backend this runtime would pick on its own. */
export function _resetPendingUploadRecordsForTesting(
  next: PendingUploadBackend | null = createMemoryPendingUploadBackend(),
): void {
  backend = next;
  owned.clear();
  live.clear();
  sending.clear();
  sendingImageHashes.value = [];
  ownedQueuedUploadSendsByThread.clear();
  othersUsage = { count: 0, bytes: 0 };
  writes = Promise.resolve();
  warned = false;
}

/** Resolves once every queued write has run. */
export function _pendingUploadWritesSettledForTesting(): Promise<void> {
  return writes;
}
