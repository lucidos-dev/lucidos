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
  pageOwnerId,
  pendingUploadDbName,
  pendingUploadStorageFailureMessage,
  persistPendingUpload,
  readPendingUploadStore,
  _resetPendingUploadRecordsForTesting,
  type PendingUploadBackend,
  type PendingUploadRecord,
} from './pendingUploadRecords';
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
