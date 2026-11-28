
import { sourceGeneration, type RestoreSourceOrigin } from '../utils/restoreSourceGeneration';
import type { AnswerKind } from './thread-events';
import type { AnswerPlacement } from '../api/client/chat';
import type { ChatRequestBody } from '../api/types';
import { WORKSPACE_ID } from '../utils/basePath';
import { BOUNDED, UNSENT_MESSAGES_DATABASE, awaitQueuedWrites, deviceDatabaseName, openDeviceDatabase } from './deviceDatabase';
import { pageOwnerId } from './pageOwner';
import { showToast } from './store';
import type { SendSettlement, UnsentMessage } from './unsentMessages';

export interface UnsentMessageRecord extends RestoreSourceOrigin {
  eventId: string;
  threadId: string;
  body: ChatRequestBody;
  settlement: SendSettlement;
  sentAt: string;
  phase: 'sending' | 'unsent';
  failedRetries: number;
  refusal?: UnsentMessage['refusal'];
  ownerId: string;
  answersQuestion?: string;
  questionPick?: { toolUseId: string; answer: AnswerKind; placement: AnswerPlacement };
}

type UnsentPatch = Pick<UnsentMessageRecord, 'phase' | 'failedRetries' | 'refusal'> | Pick<UnsentMessageRecord, 'ownerId'>;

export interface UnsentMessageBackend {
  put(record: UnsentMessageRecord): Promise<void>;
  patch(eventId: string, patch: UnsentPatch): Promise<void>;
  delete(eventId: string): Promise<void>;
  list(): Promise<UnsentMessageRecord[]>;
}

export function unsentMessageDbName(workspaceId: string | null): string {
  return deviceDatabaseName(UNSENT_MESSAGES_DATABASE, workspaceId);
}

export const UNSENT_MESSAGES_STORE = 'messages';

export function createIndexedDbUnsentMessageBackend(name: string): UnsentMessageBackend {
  const { transact } = openDeviceDatabase(name, (db) => {
    db.createObjectStore(UNSENT_MESSAGES_STORE, { keyPath: 'eventId' });
  });
  return {
    put: async (record) => { await transact(UNSENT_MESSAGES_STORE, 'readwrite', (s) => s.put(record)); },
    patch: async (eventId, patch) => {
      await transact(UNSENT_MESSAGES_STORE, 'readwrite', (s) => {
        const get = s.get(eventId);
        get.onsuccess = () => {
          if (get.result) s.put({ ...get.result, ...patch });
        };
      });
    },
    delete: async (eventId) => { await transact(UNSENT_MESSAGES_STORE, 'readwrite', (s) => s.delete(eventId)); },
    list: async () => (await transact<UnsentMessageRecord[]>(UNSENT_MESSAGES_STORE, 'readonly', (s) => s.getAll(), BOUNDED)) ?? [],
  };
}

export function createMemoryUnsentMessageBackend(): UnsentMessageBackend {
  const records = new Map<string, UnsentMessageRecord>();
  return {
    put: async (record) => { records.set(record.eventId, structuredClone(record)); },
    patch: async (eventId, patch) => {
      const record = records.get(eventId);
      if (record) records.set(eventId, { ...record, ...patch });
    },
    delete: async (eventId) => { records.delete(eventId); },
    list: async () => [...records.values()].map((r) => structuredClone(r)),
  };
}

function createUnavailableBackend(): UnsentMessageBackend {
  const fail = async (): Promise<never> => {
    throw new Error('this browser has no IndexedDB');
  };
  return { put: fail, patch: fail, delete: fail, list: fail };
}

let backend: UnsentMessageBackend | null = null;
const live = new Map<string, UnsentMessageRecord>();

export function liveUnsentMessageRecords(): UnsentMessageRecord[] {
  return [...live.values()];
}

function store(): UnsentMessageBackend {
  backend ??= typeof indexedDB === 'undefined'
    ? createUnavailableBackend()
    : createIndexedDbUnsentMessageBackend(unsentMessageDbName(WORKSPACE_ID));
  return backend;
}

let writes: Promise<void> = Promise.resolve();
let warned = false;

export function unsentMessageStorageFailureMessage(err: unknown): string {
  const reason = err instanceof Error ? err.message : String(err);
  return `Unsent messages will not survive a page reload: this browser could not save them (${reason}).`;
}

function enqueue(write: () => Promise<void>): void {
  writes = writes.then(write).catch((err) => {
    if (warned) return;
    warned = true;
    showToast(unsentMessageStorageFailureMessage(err), 'warning');
  });
}

export function persistSendingMessage(
  record: Omit<UnsentMessageRecord, 'phase' | 'failedRetries' | 'ownerId' | 'sourceGeneration'>,
): void {
  const stored: UnsentMessageRecord = { ...record, phase: 'sending', failedRetries: 0, ownerId: pageOwnerId, sourceGeneration: sourceGeneration() };
  live.set(record.eventId, stored);
  enqueue(() => store().put(stored));
}

export function markUnsentMessageRecordUnsent(eventId: string, failedRetries: number, refusal?: UnsentMessageRecord['refusal']): void {
  const record = live.get(eventId);
  if (record) live.set(eventId, { ...record, phase: 'unsent', failedRetries, refusal });
  enqueue(() => store().patch(eventId, { phase: 'unsent', failedRetries, refusal }));
}

export function adoptUnsentMessageRecord(record: UnsentMessageRecord): void {
  live.set(record.eventId, { ...record, ownerId: pageOwnerId });
  enqueue(() => store().patch(record.eventId, { ownerId: pageOwnerId }));
}

export function forgetUnsentMessageRecord(eventId: string): void {
  live.delete(eventId);
  enqueue(() => store().delete(eventId));
}

export async function readUnsentMessageStore(): Promise<UnsentMessageRecord[]> {
  await awaitQueuedWrites(writes, unsentMessageDbName(WORKSPACE_ID));
  return store().list();
}

export function _resetUnsentMessageRecordsForTesting(
  next: UnsentMessageBackend | null = createMemoryUnsentMessageBackend(),
): void {
  backend = next;
  live.clear();
  writes = Promise.resolve();
  warned = false;
}
