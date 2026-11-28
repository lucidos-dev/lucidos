
import { sourceGeneration, type RestoreSourceOrigin } from '../utils/restoreSourceGeneration';
import type { UploadSendIntent } from './uploadSendIntents';
import { signal } from '@preact/signals';
import { WORKSPACE_ID } from '../utils/basePath';
import { PENDING_UPLOADS_DATABASE, awaitQueuedWrites, deviceDatabaseName } from './deviceDatabase';
import { createIndexedDbBackend } from './pendingUploadBackend';
export { createMemoryPendingUploadBackend } from './pendingUploadBackend';
import { createMemoryPendingUploadBackend } from './pendingUploadBackend';
import type { StoredUploadSource } from './pendingUploadSource';
import { pageOwnerId } from './pageOwner';
import { showToast } from './store';

export interface PendingUploadRecord extends RestoreSourceOrigin {
  localId: string;
  threadId: string;
  name: string;
  mime: string;
  bytes: ArrayBuffer;
  migratedBytes?: true;
  size: number;
  createdAt: number;
  ownerId: string;
  landedHash?: string;
}

export interface QueuedUploadSendRecord extends RestoreSourceOrigin {
  threadId: string;
  intent: UploadSendIntent;
  ownerId: string;
  localIds: string[];
  text: string;
}

export type QueuedUploadSendSnapshot = Pick<QueuedUploadSendRecord, 'localIds' | 'text'>;

type UploadPatch = Partial<Pick<PendingUploadRecord, 'landedHash' | 'ownerId'>>;

export interface PendingUploadBackend {
  putUpload(record: PendingUploadRecord): Promise<void>;
  patchUpload(localId: string, patch: UploadPatch): Promise<void>;
  deleteUpload(localId: string): Promise<void>;
  listUploadHistory(): Promise<StoredUploadSource[]>;
  readUploadBytes(source: StoredUploadSource): Promise<ArrayBuffer | undefined>;
  listUploads(): Promise<PendingUploadRecord[]>;
  putQueuedUploadSend(record: QueuedUploadSendRecord): Promise<void>;
  deleteQueuedUploadSend(record: QueuedUploadSendRecord): Promise<void>;
  listQueuedUploadHistory(): Promise<QueuedUploadSendRecord[]>;
  listQueuedUploadSends(): Promise<QueuedUploadSendRecord[]>;
}

export const PENDING_UPLOAD_MAX_RECORDS = 20;
export const PENDING_UPLOAD_MAX_BYTES = 100 * 1024 * 1024;
export const PENDING_UPLOAD_MAX_AGE_MS = 7 * 24 * 60 * 60 * 1000;

export function pendingUploadDbName(workspaceId: string | null): string {
  return deviceDatabaseName(PENDING_UPLOADS_DATABASE, workspaceId);
}

const DB_NAME = pendingUploadDbName(WORKSPACE_ID);
function createUnavailableBackend(): PendingUploadBackend {
  const fail = async (): Promise<never> => {
    throw new Error('this browser has no IndexedDB');
  };
  return {
    putUpload: fail, patchUpload: fail, deleteUpload: fail, listUploads: fail, listUploadHistory: fail, readUploadBytes: fail,
    putQueuedUploadSend: fail, deleteQueuedUploadSend: fail, listQueuedUploadSends: fail, listQueuedUploadHistory: fail,
  };
}

let backend: PendingUploadBackend | null = null;

function store(): PendingUploadBackend {
  backend ??= typeof indexedDB === 'undefined' ? createUnavailableBackend() : createIndexedDbBackend(DB_NAME);
  return backend;
}

export interface OwnedRecord {
  localId: string;
  threadId: string;
  size: number;
  landedHash?: string;
}

const owned = new Map<string, OwnedRecord>();
const live = new Map<string, PendingUploadRecord>();

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
const ownedQueuedUploadSendsByThread = new Map<string, QueuedUploadSendRecord>();
let othersUsage = { count: 0, bytes: 0 };

let writes: Promise<void> = Promise.resolve();
let warned = false;

export const PENDING_UPLOAD_OVER_CAP_MESSAGE =
  'This image will not survive a page reload: too many images are waiting to upload. It is still uploading.';

export function pendingUploadStorageFailureMessage(err: unknown): string {
  const reason = err instanceof Error ? err.message : String(err);
  return `Images waiting to upload will not survive a page reload: this browser could not save them (${reason}).`;
}

function warnOnce(message: string): void {
  if (warned) return;
  warned = true;
  showToast(message, 'warning');
}

function enqueue(write: () => Promise<void>): void {
  writes = writes.then(write).catch((err) => warnOnce(pendingUploadStorageFailureMessage(err)));
}

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

export function markPendingUploadLanded(localId: string, hash: string): void {
  const inMemory = live.get(localId);
  if (inMemory) live.set(localId, { ...inMemory, landedHash: hash });
  const record = owned.get(localId);
  if (!record) return;
  owned.set(localId, { ...record, landedHash: hash });
  enqueue(() => store().patchUpload(localId, { landedHash: hash }));
}

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

export async function readPendingUploadHistory(): Promise<{
  uploads: StoredUploadSource[];
  queuedUploadSendRecords: QueuedUploadSendRecord[];
}> {
  await awaitQueuedWrites(writes, DB_NAME);
  const [uploads, queuedUploadSendRecords] = await Promise.all([store().listUploadHistory(), store().listQueuedUploadHistory()]);
  return { uploads, queuedUploadSendRecords };
}

export function readArchivedUploadBytes(source: StoredUploadSource): Promise<ArrayBuffer | undefined> {
  return store().readUploadBytes(source);
}

export async function readPendingUploadStore(): Promise<{
  uploads: PendingUploadRecord[];
  queuedUploadSendRecords: QueuedUploadSendRecord[];
}> {
  await awaitQueuedWrites(writes, DB_NAME);
  const [uploads, queuedUploadSendRecords] = await Promise.all([store().listUploads(), store().listQueuedUploadSends()]);
  return { uploads, queuedUploadSendRecords };
}

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

export function _pendingUploadWritesSettledForTesting(): Promise<void> {
  return writes;
}
