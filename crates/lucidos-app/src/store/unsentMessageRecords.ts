/** Every send whose outcome this page has not yet learned, kept on this device
 *  so a page reload does not lose it. A record is written when the send
 *  starts and deleted once the engine takes or refuses it. One found at
 *  startup is therefore an *unsent message*. The live ones are in
 *  `store/unsentMessages.ts`; the restore is `actions/unsentMessageRestore.ts`.
 *
 *  Plan: `docs/plans/2026-10-03-unsent-messages-survive-a-reload.md`. */

import type { ChatRequestBody } from '../api/types';
import { WORKSPACE_ID } from '../utils/basePath';
import { deviceDatabaseName, openDeviceDatabase } from './deviceDatabase';
import { pageOwnerId } from './pageOwner';
import { showToast } from './store';
import type { SendSettlement } from './unsentMessages';

export interface UnsentMessageRecord {
  /** The send's client event id: the key, and the exchange's starter id. */
  eventId: string;
  threadId: string;
  /** The exact request, so a Retry after the reload re-posts it unchanged. */
  body: ChatRequestBody;
  settlement: SendSettlement;
  /** When it was sent, so the restored card sorts where it was. */
  sentAt: string;
  /** `sending` until the first attempt got no answer. A reload in between
   *  leaves it unanswered all the same. */
  phase: 'sending' | 'unsent';
  /** Retries that also got no answer. */
  failedRetries: number;
  /** The page load that owns the record (`store/pageOwner.ts`). */
  ownerId: string;
}

type UnsentPatch = Pick<UnsentMessageRecord, 'phase' | 'failedRetries'> | Pick<UnsentMessageRecord, 'ownerId'>;

export interface UnsentMessageBackend {
  put(record: UnsentMessageRecord): Promise<void>;
  /** Changes a record that exists, and never creates one. */
  patch(eventId: string, patch: UnsentPatch): Promise<void>;
  delete(eventId: string): Promise<void>;
  list(): Promise<UnsentMessageRecord[]>;
}

export function unsentMessageDbName(workspaceId: string | null): string {
  return deviceDatabaseName('lucidos-unsent-messages', workspaceId);
}

const MESSAGES = 'messages';

function createIndexedDbBackend(name: string): UnsentMessageBackend {
  const { transact } = openDeviceDatabase(name, (db) => {
    db.createObjectStore(MESSAGES, { keyPath: 'eventId' });
  });
  return {
    put: async (record) => { await transact(MESSAGES, 'readwrite', (s) => s.put(record)); },
    patch: async (eventId, patch) => {
      await transact(MESSAGES, 'readwrite', (s) => {
        const get = s.get(eventId);
        get.onsuccess = () => {
          if (get.result) s.put({ ...get.result, ...patch });
        };
      });
    },
    delete: async (eventId) => { await transact(MESSAGES, 'readwrite', (s) => s.delete(eventId)); },
    list: async () => (await transact<UnsentMessageRecord[]>(MESSAGES, 'readonly', (s) => s.getAll())) ?? [],
  };
}

/** The same contract over a map, for tests. Vitest runs under Node, which has
 *  no IndexedDB. */
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

/** Every operation fails, so a browser without IndexedDB hears that its
 *  unsent messages will not survive a reload. */
function createUnavailableBackend(): UnsentMessageBackend {
  const fail = async (): Promise<never> => {
    throw new Error('this browser has no IndexedDB');
  };
  return { put: fail, patch: fail, delete: fail, list: fail };
}

let backend: UnsentMessageBackend | null = null;

function store(): UnsentMessageBackend {
  backend ??= typeof indexedDB === 'undefined'
    ? createUnavailableBackend()
    : createIndexedDbBackend(unsentMessageDbName(WORKSPACE_ID));
  return backend;
}

/** Writes run one at a time, in order, so a delete never overtakes its put. */
let writes: Promise<void> = Promise.resolve();
let warned = false;

export function unsentMessageStorageFailureMessage(err: unknown): string {
  const reason = err instanceof Error ? err.message : String(err);
  return `Unsent messages will not survive a page reload: this browser could not save them (${reason}).`;
}

/** Never awaited by a send: storage must not hold up or fail one. A failure is
 *  said once per page, and the send carries on in memory. */
function enqueue(write: () => Promise<void>): void {
  writes = writes.then(write).catch((err) => {
    if (warned) return;
    warned = true;
    showToast(unsentMessageStorageFailureMessage(err), 'warning');
  });
}

/** Keep a send that is about to post. */
export function persistSendingMessage(
  record: Omit<UnsentMessageRecord, 'phase' | 'failedRetries' | 'ownerId'>,
): void {
  const stored: UnsentMessageRecord = { ...record, phase: 'sending', failedRetries: 0, ownerId: pageOwnerId };
  enqueue(() => store().put(stored));
}

/** The send, or a Retry of it, got no answer. */
export function markUnsentMessageRecordUnsent(eventId: string, failedRetries: number): void {
  enqueue(() => store().patch(eventId, { phase: 'unsent', failedRetries }));
}

/** Take over a record a previous page load left. */
export function adoptUnsentMessageRecord(record: UnsentMessageRecord): void {
  enqueue(() => store().patch(record.eventId, { ownerId: pageOwnerId }));
}

/** The send was decided, or the user discarded it. */
export function forgetUnsentMessageRecord(eventId: string): void {
  enqueue(() => store().delete(eventId));
}

/** Everything stored, after every write this page has queued. A failed read
 *  rejects, so the caller can tell "nothing stored" from "could not look". */
export async function readUnsentMessageStore(): Promise<UnsentMessageRecord[]> {
  await writes;
  return store().list();
}

/** `null` puts back the backend this runtime would pick on its own. */
export function _resetUnsentMessageRecordsForTesting(
  next: UnsentMessageBackend | null = createMemoryUnsentMessageBackend(),
): void {
  backend = next;
  writes = Promise.resolve();
  warned = false;
}
