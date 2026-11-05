import { observeSourceGeneration } from '../utils/restoreSourceGeneration';
import { describe, it, expect, beforeEach } from 'vitest';

import {
  PENDING_UPLOAD_MAX_BYTES,
  PENDING_UPLOAD_MAX_RECORDS,
  PENDING_UPLOAD_OVER_CAP_MESSAGE,
  adoptPendingUploadRecord,
  createMemoryPendingUploadBackend,
  forgetPendingUploadRecord,
  markPendingUploadLanded,
  notePendingUploadsOwnedElsewhere,
  ownedPendingUploadRecords,
  pendingUploadDbName,
  pendingUploadStorageFailureMessage,
  persistPendingUpload,
  readPendingUploadStore,
  livePendingUploadRecords,
  _resetPendingUploadRecordsForTesting,
  type PendingUploadBackend,
  type PendingUploadRecord,
} from './pendingUploadRecords';
import { pageOwnerId } from './pageOwner';
import { toasts } from './store';

const bytes = (n: number) => new Uint8Array(n).buffer;

function persist(localId: string, size = 3): boolean {
  return persistPendingUpload({ localId, threadId: 't-1', name: 'a.png', mime: 'image/png', bytes: bytes(size) });
}

function record(localId: string, overrides: Partial<PendingUploadRecord> = {}): PendingUploadRecord {
  return {
    localId, threadId: 't-1', name: 'a.png', mime: 'image/png', bytes: bytes(3), size: 3,
    createdAt: Date.now(), ownerId: 'an-earlier-page', ...overrides,
  };
}

beforeEach(() => {
  observeSourceGeneration(null);
  _resetPendingUploadRecordsForTesting();
  toasts.value = [];
});

describe('pendingUploadDbName', () => {
  it('gives each workspace its own database, since workspaces share one origin', () => {
    expect(pendingUploadDbName('alpha')).not.toBe(pendingUploadDbName('beta'));
    expect(pendingUploadDbName('alpha')).toBe('lucidos-pending-uploads:alpha');
    expect(pendingUploadDbName(null)).toBe('lucidos-pending-uploads');
  });
});

describe('persistPendingUpload', () => {
  it('retains landed live bytes excluded by the durable cap until explicitly released', async () => {
    notePendingUploadsOwnedElsewhere(Array.from({ length: PENDING_UPLOAD_MAX_RECORDS }, (_, i) => record(`other-${i}`)));
    expect(persist('over-cap')).toBe(false);
    markPendingUploadLanded('over-cap', 'confirmed-hash');
    expect((await readPendingUploadStore()).uploads).toEqual([]);
    expect(livePendingUploadRecords()).toEqual([expect.objectContaining({ localId: 'over-cap', landedHash: 'confirmed-hash', bytes: bytes(3) })]);
    expect(ownedPendingUploadRecords()).toEqual([expect.objectContaining({ localId: 'over-cap', landedHash: 'confirmed-hash' })]);
    forgetPendingUploadRecord('over-cap');
    expect(livePendingUploadRecords()).toEqual([]);
  });

  it('retains live bytes when IndexedDB rejects the write', async () => {
    _resetPendingUploadRecordsForTesting({ ...createMemoryPendingUploadBackend(), putUpload: async () => { throw new Error('quota'); } });
    persist('failed-write');
    await readPendingUploadStore();
    markPendingUploadLanded('failed-write', 'confirmed-hash');
    expect(livePendingUploadRecords()[0]).toMatchObject({ localId: 'failed-write', landedHash: 'confirmed-hash', bytes: bytes(3) });
  });

  it('stores the bytes and metadata, owned by this page', async () => {
    expect(persist('u-1')).toBe(true);
    const { uploads } = await readPendingUploadStore();
    expect(uploads).toHaveLength(1);
    expect(uploads[0]).toMatchObject({ localId: 'u-1', threadId: 't-1', name: 'a.png', mime: 'image/png', size: 3, ownerId: pageOwnerId });
    expect(uploads[0].bytes.byteLength).toBe(3);
  });

  it('runs writes in order, so a delete right after the put wins', async () => {
    persist('u-1');
    forgetPendingUploadRecord('u-1');
    expect((await readPendingUploadStore()).uploads).toEqual([]);
  });

  it('marks a landed upload with its hash', async () => {
    persist('u-1');
    markPendingUploadLanded('u-1', 'h-1');
    expect((await readPendingUploadStore()).uploads[0].landedHash).toBe('h-1');
    expect(ownedPendingUploadRecords()[0].landedHash).toBe('h-1');
  });

  it(`refuses a record past ${PENDING_UPLOAD_MAX_RECORDS}, and says so once`, async () => {
    for (let i = 0; i < PENDING_UPLOAD_MAX_RECORDS; i++) expect(persist(`u-${i}`)).toBe(true);
    expect(persist('one-too-many')).toBe(false);
    expect(persist('another')).toBe(false);
    expect((await readPendingUploadStore()).uploads).toHaveLength(PENDING_UPLOAD_MAX_RECORDS);
    expect(toasts.value.map((t) => t.message)).toEqual([PENDING_UPLOAD_OVER_CAP_MESSAGE]);
  });

  it('refuses a record that would cross the byte cap', () => {
    expect(persist('big', PENDING_UPLOAD_MAX_BYTES - 10)).toBe(true);
    expect(persist('small', 11)).toBe(false);
    expect(persist('fits', 10)).toBe(true);
  });

  it("counts other tabs' records against the caps", () => {
    notePendingUploadsOwnedElsewhere(
      Array.from({ length: PENDING_UPLOAD_MAX_RECORDS }, (_, i) => record(`other-${i}`)),
    );
    expect(persist('u-1')).toBe(false);
  });

  it('says once when the browser cannot store, and the caller carries on', async () => {
    const failing: PendingUploadBackend = {
      ...createMemoryPendingUploadBackend(),
      putUpload: async () => { throw new DOMException('quota', 'QuotaExceededError'); },
    };
    _resetPendingUploadRecordsForTesting(failing);
    expect(persist('u-1')).toBe(true);
    expect(persist('u-2')).toBe(true);
    await readPendingUploadStore();
    const messages = toasts.value.map((t) => t.message);
    expect(messages).toEqual([pendingUploadStorageFailureMessage(new DOMException('quota', 'QuotaExceededError'))]);
  });
});

describe('a browser without IndexedDB', () => {
  it('hears once that images will not survive a reload, rather than a silent memory store', async () => {
    _resetPendingUploadRecordsForTesting(null);
    expect(persist('u-1')).toBe(true);
    await expect(readPendingUploadStore()).rejects.toThrow('this browser has no IndexedDB');
    expect(toasts.value.map((t) => t.message)).toEqual([
      pendingUploadStorageFailureMessage(new Error('this browser has no IndexedDB')),
    ]);
  });
});

describe('adoptPendingUploadRecord', () => {
  it('takes over an earlier page load’s record and resumes it as unlanded', async () => {
    const backend = createMemoryPendingUploadBackend();
    await backend.putUpload(record('u-1', { landedHash: 'h-1' }));
    _resetPendingUploadRecordsForTesting(backend);
    const [stored] = (await readPendingUploadStore()).uploads;
    adoptPendingUploadRecord(stored);
    const [after] = (await readPendingUploadStore()).uploads;
    expect(after.ownerId).toBe(pageOwnerId);
    expect(after.landedHash).toBeUndefined();
    expect(ownedPendingUploadRecords().map((r) => r.localId)).toEqual(['u-1']);
  });
});

it('stamps uploads and queued sends synchronously, retaining origin through adoption', async () => {
  const { persistQueuedUploadSend, adoptQueuedUploadSendRecord } = await import('./pendingUploadRecords');
  observeSourceGeneration('old');
  persist('tagged');
  persistQueuedUploadSend('t-1', { useCodingAgent: false, context: null }, { localIds: ['tagged'], text: 'before' });
  observeSourceGeneration('current');
  const stored = await readPendingUploadStore();
  expect(stored.uploads[0].sourceGeneration).toBe('old');
  expect(stored.queuedUploadSendRecords[0].sourceGeneration).toBe('old');
  adoptPendingUploadRecord(stored.uploads[0]);
  adoptQueuedUploadSendRecord(stored.queuedUploadSendRecords[0]);
  const adopted = await readPendingUploadStore();
  expect(adopted.uploads[0].sourceGeneration).toBe('old');
  expect(adopted.queuedUploadSendRecords[0].sourceGeneration).toBe('old');
});


describe('queued source isolation', () => {
  it('keeps owners and generations on the same thread and drops settled history', async () => {
    const backend = createMemoryPendingUploadBackend();
    const old = { threadId: 'same', ownerId: 'old-page', sourceGeneration: 'before',
      localIds: ['image'], text: 'old', intent: { useCodingAgent: false, context: null } };
    const current = { ...old, ownerId: 'current-page', sourceGeneration: 'after', text: 'current' };
    await Promise.all([backend.putQueuedUploadSend(old), backend.putQueuedUploadSend(current)]);
    expect(await backend.listQueuedUploadSends()).toEqual([old, current]);
    await backend.deleteQueuedUploadSend(old);
    expect(await backend.listQueuedUploadSends()).toEqual([current]);
    expect(await backend.listQueuedUploadHistory()).toEqual([current]);
  });

  it('refuses stale settlement of a newer intent, keeping only the live revision', async () => {
    const backend = createMemoryPendingUploadBackend();
    const first = { threadId: 'same', ownerId: 'page', sourceGeneration: 'current',
      localIds: ['image'], text: 'first', intent: { useCodingAgent: false, context: null } };
    const next = { ...first, text: 'edited' };
    await backend.putQueuedUploadSend(first);
    await backend.putQueuedUploadSend(next);
    expect(await backend.listQueuedUploadHistory()).toEqual([next]);
    await backend.deleteQueuedUploadSend(first);
    expect(await backend.listQueuedUploadSends()).toEqual([next]);
    expect(await backend.listQueuedUploadHistory()).toEqual([next]);
    await backend.deleteQueuedUploadSend(next);
    expect(await backend.listQueuedUploadSends()).toEqual([]);
    expect(await backend.listQueuedUploadHistory()).toEqual([]);
  });
});


describe('retained attachment sources', () => {
  // History holds intent a restore could strand. A deleted upload was delivered
  // or dropped, so keeping its bytes would only grow device storage forever.
  it('keeps bytes and owner lineage after patch and overwrite, and drops them on deletion', async () => {
    const backend = createMemoryPendingUploadBackend();
    _resetPendingUploadRecordsForTesting(backend);
    const old = record('image', { sourceGeneration: 'before' });
    await backend.putUpload(old);
    await backend.patchUpload('image', { ownerId: 'adopter', landedHash: 'landed' });
    const current = record('image', { ownerId: 'current', sourceGeneration: 'after', bytes: bytes(5), size: 5 });
    await backend.putUpload(current);
    expect((await readPendingUploadStore(true)).uploads).toEqual([
      old, { ...old, ownerId: 'adopter', landedHash: 'landed' }, current,
    ]);
    await backend.putUpload(record('other'));
    await backend.deleteUpload('image');
    expect((await readPendingUploadStore()).uploads.map((u) => u.localId)).toEqual(['other']);
    expect((await readPendingUploadStore(true)).uploads.map((u) => u.localId)).toEqual(['other']);
  });
});
