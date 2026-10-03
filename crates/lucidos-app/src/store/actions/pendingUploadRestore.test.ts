import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';

(globalThis as any).URL.createObjectURL = () => 'blob:fake';
(globalThis as any).URL.revokeObjectURL = () => {};

vi.mock('./compose', async (importOriginal) => {
  const actual = await importOriginal<typeof import('./compose')>();
  return {
    ...actual,
    awaitThreadStarted: vi.fn(async () => {}),
    isThreadStartPending: vi.fn(() => false),
  };
});

vi.mock('./thread-loading', async (importOriginal) => {
  const actual = await importOriginal<typeof import('./thread-loading')>();
  return { ...actual, ensureThreadByIdInMap: vi.fn() };
});

vi.mock('../../api/client', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../../api/client')>();
  return { ...actual, uploadThreadBlob: vi.fn() };
});

import {
  droppedImagesMessage,
  installPendingUploadRestore,
  restorePendingUploads,
  _resetPendingUploadRestoreForTesting,
} from './pendingUploadRestore';
import { ensureThreadByIdInMap } from './thread-loading';
import { noteServerDraft } from './compose';
import { _resetImageUploadRunsForTesting } from './imageUploads';
import { ApiError, uploadThreadBlob } from '../../api/client';
import {
  PENDING_UPLOAD_MAX_AGE_MS,
  createMemoryPendingUploadBackend,
  readPendingUploadStore,
  _resetPendingUploadRecordsForTesting,
  type PendingUploadBackend,
  type PendingUploadRecord,
} from '../pendingUploadRecords';
import { PAGE_OWNER_LOCK_PREFIX } from '../pageOwner';
import {
  addPendingUpload,
  detachPendingUpload,
  getPendingUploads,
  _resetPendingUploadsForTesting,
} from '../pendingUploads';
import { connectionStatus, threadMap, toasts } from '../store';
import { getDraft, setDraft, _resetComposeDraftsForTesting } from '../composeDrafts';
import { _resetSessionBlobUrlsForTesting } from '../../components/chat/pastedImages';
import {
  clearQueuedUploadSend,
  queueUploadSend,
  queuedUploadSends,
} from '../../components/chat/prompt-input-helpers';
import { makeOptimisticThreadState, type ThreadState } from '../thread-events';

const LIVE = 't-live';
const NOW = 1_000_000_000_000;
const EARLIER_PAGE = 'an-earlier-page';

function record(localId: string, overrides: Partial<PendingUploadRecord> = {}): PendingUploadRecord {
  return {
    localId, threadId: LIVE, name: 'photo.png', mime: 'image/png',
    bytes: new Uint8Array([1, 2, 3]).buffer, size: 3,
    createdAt: NOW - 60_000, ownerId: EARLIER_PAGE, ...overrides,
  };
}

function thread(id: string, state: 'composing' | 'active' | 'discarded' = 'composing'): ThreadState {
  return makeOptimisticThreadState({
    id, title: '', channel: 'chat', initiator: 'user', eventsLoaded: true, state, status: 'idle',
  });
}

let backend: PendingUploadBackend;
const originalFetch = globalThis.fetch;

async function seed(...records: PendingUploadRecord[]): Promise<void> {
  for (const r of records) await backend.putUpload(r);
}

const storedIds = async () => (await readPendingUploadStore()).uploads.map((r) => r.localId).sort();
const warnings = () => toasts.value.filter((t) => t.type === 'warning').map((t) => t.message);
const chipIds = (threadId = LIVE) => getPendingUploads(threadId).map((u) => u.localId);

beforeEach(() => {
  backend = createMemoryPendingUploadBackend();
  _resetPendingUploadRecordsForTesting(backend);
  _resetPendingUploadRestoreForTesting();
  toasts.value = [];
  connectionStatus.value = 'connected';
  threadMap.value = new Map([[LIVE, thread(LIVE)]]);
  noteServerDraft(LIVE, '', []);
  queuedUploadSends.value = new Map();
  vi.mocked(ensureThreadByIdInMap).mockReset().mockImplementation(async (id) => threadMap.value.has(id));
  // Held, so a resumed upload stays a chip for the test to see.
  vi.mocked(uploadThreadBlob).mockReset().mockImplementation(() => new Promise(() => {}));
});

afterEach(() => {
  globalThis.fetch = originalFetch;
  _resetImageUploadRunsForTesting();
  _resetPendingUploadsForTesting();
  _resetComposeDraftsForTesting();
  _resetSessionBlobUrlsForTesting();
  queuedUploadSends.value = new Map();
  threadMap.value = new Map();
  connectionStatus.value = 'disconnected';
  Reflect.deleteProperty(globalThis.navigator as object, 'locks');
});

describe('restorePendingUploads', () => {
  it('brings an image back as a chip on its live draft, and resumes the upload', async () => {
    await seed(record('u-1'));
    const outcome = await restorePendingUploads(NOW);

    expect(outcome.restored).toBe(1);
    const [chip] = getPendingUploads(LIVE);
    expect(chip.localId).toBe('u-1');
    expect(chip.mime).toBe('image/png');
    expect(chip.file.name).toBe('photo.png');
    expect(new Uint8Array(await chip.file.arrayBuffer())).toEqual(new Uint8Array([1, 2, 3]));
    expect(uploadThreadBlob).toHaveBeenCalledWith(LIVE, chip.file, expect.anything());
    // Now this page's record.
    expect((await readPendingUploadStore()).uploads[0].ownerId).not.toBe(EARLIER_PAGE);
    expect(warnings()).toEqual([]);
  });

  it('the resumed upload lands in the draft like any other', async () => {
    vi.mocked(uploadThreadBlob).mockResolvedValue({ hash: 'h1', mime: 'image/png', byte_size: 3 });
    globalThis.fetch = vi.fn(() => new Promise<Response>(() => {})) as unknown as typeof fetch;
    await seed(record('u-1'));
    await restorePendingUploads(NOW);
    await vi.waitFor(() => expect(getDraft(LIVE).image_hashes).toEqual(['h1']));
    expect(chipIds()).toEqual([]);
  });

  it('looks up a draft outside the loaded thread list instead of dropping it', async () => {
    vi.mocked(ensureThreadByIdInMap).mockImplementation(async (id) => {
      threadMap.value = new Map([...threadMap.value, [id, thread(id, 'active')]]);
      return true;
    });
    await seed(record('u-old', { threadId: 't-old' }));
    const outcome = await restorePendingUploads(NOW);
    expect(outcome.restored).toBe(1);
    expect(chipIds('t-old')).toEqual(['u-old']);
  });

  it('drops an image whose draft the engine no longer has, and says so', async () => {
    vi.mocked(ensureThreadByIdInMap).mockResolvedValue(false);
    await seed(record('u-1', { threadId: 't-gone' }));
    const outcome = await restorePendingUploads(NOW);
    expect(outcome.gone).toBe(1);
    expect(await storedIds()).toEqual([]);
    expect(warnings()).toEqual(['An image attached before the reload was dropped: its draft no longer exists.']);
  });

  it('drops an image whose draft was discarded', async () => {
    threadMap.value = new Map([[LIVE, thread(LIVE, 'discarded')]]);
    await seed(record('u-1'), record('u-2'));
    await restorePendingUploads(NOW);
    expect(await storedIds()).toEqual([]);
    expect(chipIds()).toEqual([]);
    expect(warnings()).toEqual(['2 images attached before the reload were dropped: their drafts no longer exist.']);
  });

  it('keeps a record whose draft could not be checked, says so, and restores it on a later pass', async () => {
    vi.mocked(ensureThreadByIdInMap).mockRejectedValueOnce(new ApiError(500, 'database unavailable'));
    await seed(record('u-1', { threadId: 't-unknown' }));
    const first = await restorePendingUploads(NOW);
    expect(first.unresolved).toBe(1);
    expect(await storedIds()).toEqual(['u-1']);
    expect(warnings()).toHaveLength(1);
    expect(warnings()[0]).toMatch(/^An image attached before the reload could not be brought back yet \(.*database unavailable.*\)\. Lucidos tries again/);

    vi.mocked(ensureThreadByIdInMap).mockImplementation(async (id) => {
      threadMap.value = new Map([...threadMap.value, [id, thread(id)]]);
      return true;
    });
    const second = await restorePendingUploads(NOW);
    expect(second.restored).toBe(1);
    expect(chipIds('t-unknown')).toEqual(['u-1']);
  });

  it('drops an image older than the age cap, and says why', async () => {
    await seed(record('u-1', { createdAt: NOW - PENDING_UPLOAD_MAX_AGE_MS - 1 }));
    const outcome = await restorePendingUploads(NOW);
    expect(outcome.expired).toBe(1);
    expect(await storedIds()).toEqual([]);
    expect(warnings()).toEqual(['An image attached before the reload was dropped: it waited more than 7 days.']);
  });

  it('forgets a landed image the draft already holds, quietly', async () => {
    setDraft(LIVE, { text: '', image_hashes: ['h1'], mode: 'lucidos' });
    await seed(record('u-1', { landedHash: 'h1' }));
    const outcome = await restorePendingUploads(NOW);
    expect(outcome.restored).toBe(0);
    expect(await storedIds()).toEqual([]);
    expect(chipIds()).toEqual([]);
    expect(warnings()).toEqual([]);
  });

  it('re-uploads a landed image the draft never received', async () => {
    await seed(record('u-1', { landedHash: 'h1' }));
    await restorePendingUploads(NOW);
    expect(chipIds()).toEqual(['u-1']);
    expect((await readPendingUploadStore()).uploads[0].landedHash).toBeUndefined();
  });

  it("leaves another open tab's images alone", async () => {
    const prefix = PAGE_OWNER_LOCK_PREFIX;
    Object.defineProperty(globalThis.navigator, 'locks', {
      configurable: true,
      value: { query: async () => ({ held: [{ name: prefix + 'other-tab' }] }), request: async () => {} },
    });
    // Their draft would read as gone here, so a wrong adopt would drop it.
    await seed(record('theirs', { ownerId: 'other-tab', threadId: 't-gone' }), record('orphan'));
    await restorePendingUploads(NOW);
    expect(await storedIds()).toEqual(['orphan', 'theirs']);
    expect(chipIds()).toEqual(['orphan']);
    expect(warnings()).toEqual([]);
  });
});

describe('a send queued at reload time', () => {
  const intent = { useCodingAgent: false, context: { app_context: { app_id: 'habit-tracker' } } };
  const queuedOn = (localIds: string[], text = '') =>
    backend.putQueuedUploadSend({ threadId: LIVE, intent, ownerId: EARLIER_PAGE, localIds, text });
  const NOT_SENT = 'A send queued before the reload was not sent: an image is lost or the draft changed. Check the draft, then send.';

  it('is queued again when every image it waited on comes back', async () => {
    await seed(record('u-1'), record('u-2'));
    await queuedOn(['u-1', 'u-2']);
    await restorePendingUploads(NOW);
    expect(queuedUploadSends.value.get(LIVE)).toEqual(intent);
    // Still stored while it waits, so a second reload keeps it too.
    expect((await readPendingUploadStore()).queuedUploadSendRecords.map((q) => q.threadId)).toEqual([LIVE]);
  });

  it('is reported, not restored, when its draft is gone', async () => {
    threadMap.value = new Map([[LIVE, thread(LIVE, 'discarded')]]);
    await seed(record('u-1'));
    await queuedOn(['u-1']);
    await restorePendingUploads(NOW);
    expect(queuedUploadSends.value.has(LIVE)).toBe(false);
    expect((await readPendingUploadStore()).queuedUploadSendRecords).toEqual([]);
    expect(warnings()).toEqual([
      `An image attached before the reload was dropped: its draft no longer exists. ${NOT_SENT}`,
    ]);
  });

  it('is not restored when one of its images expired, so it never goes out missing it', async () => {
    await seed(record('u-1'), record('u-old', { createdAt: NOW - PENDING_UPLOAD_MAX_AGE_MS - 1 }));
    await queuedOn(['u-1', 'u-old']);
    await restorePendingUploads(NOW);
    expect(queuedUploadSends.value.has(LIVE)).toBe(false);
    // The image that did come back is still there for the user to send.
    expect(chipIds()).toEqual(['u-1']);
    expect(warnings()).toEqual([
      `An image attached before the reload was dropped: it waited more than 7 days. ${NOT_SENT}`,
    ]);
  });

  it('is not restored when the draft the engine serves reads differently', async () => {
    setDraft(LIVE, { text: 'an older revision', image_hashes: [], mode: 'lucidos' });
    await seed(record('u-1'));
    await queuedOn(['u-1'], 'what the user pressed Send on');
    await restorePendingUploads(NOW);
    expect(queuedUploadSends.value.has(LIVE)).toBe(false);
    expect(chipIds()).toEqual(['u-1']);
    expect(warnings()).toEqual([NOT_SENT]);
  });

  it('is not restored when an image it waited on was never stored', async () => {
    await seed(record('u-1'));
    await queuedOn(['u-1', 'over-the-cap']);
    await restorePendingUploads(NOW);
    expect(queuedUploadSends.value.has(LIVE)).toBe(false);
    expect(chipIds()).toEqual(['u-1']);
    expect(warnings()).toEqual([NOT_SENT]);
  });

  it('is stored the moment it is queued, with the images it waits on, and keeps that list current', async () => {
    const stop = installPendingUploadRestore();
    addPendingUpload({
      localId: 'u-1', threadId: LIVE, previewUrl: 'blob:u-1', mime: 'image/png',
      state: { kind: 'uploading', sentBytes: 0, totalBytes: 3 },
      file: new File([new Uint8Array([1, 2, 3])], 'a.png', { type: 'image/png' }),
    });
    setDraft(LIVE, { text: 'look at this', image_hashes: [], mode: 'lucidos' });
    queueUploadSend(LIVE, intent);
    expect((await readPendingUploadStore()).queuedUploadSendRecords)
      .toMatchObject([{ threadId: LIVE, intent, localIds: ['u-1'], text: 'look at this' }]);
    setDraft(LIVE, { text: 'look at this one', image_hashes: [], mode: 'lucidos' });
    detachPendingUpload(LIVE, 'u-1');
    expect((await readPendingUploadStore()).queuedUploadSendRecords)
      .toMatchObject([{ threadId: LIVE, localIds: [], text: 'look at this one' }]);
    clearQueuedUploadSend(LIVE);
    expect((await readPendingUploadStore()).queuedUploadSendRecords).toEqual([]);
    stop();
  });

  it('a cancelled restored send leaves the store', async () => {
    const stop = installPendingUploadRestore();
    await seed(record('u-1'));
    await queuedOn(['u-1']);
    await restorePendingUploads(NOW);
    clearQueuedUploadSend(LIVE);
    expect((await readPendingUploadStore()).queuedUploadSendRecords).toEqual([]);
    stop();
  });
});

describe('droppedImagesMessage', () => {
  it('says nothing when nothing was lost', () => {
    expect(droppedImagesMessage({ gone: 0, expired: 0, unsentQueuedUploadSends: 0, unresolved: 0 })).toBeNull();
  });

  it('names both reasons when both apply', () => {
    expect(droppedImagesMessage({ gone: 1, expired: 1, unsentQueuedUploadSends: 2, unresolved: 0 })).toBe(
      '2 images attached before the reload were dropped: their drafts are gone or they waited more than 7 days.'
      + ' 2 sends queued before the reload were not sent: an image is lost or the draft changed. Check the draft, then send.',
    );
  });
});
