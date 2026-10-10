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

vi.mock('../../api/client', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../../api/client')>();
  return { ...actual, uploadThreadBlob: vi.fn() };
});

import {
  UPLOAD_RETRY_DELAYS_MS,
  attachPendingUpload,
  cancelPendingUpload,
  recordStillNeeded,
  resumePendingUpload,
  retryPendingUpload,
  runPendingUpload,
  stateAfterFailure,
  _resetImageUploadRunsForTesting,
} from './imageUploads';
import { ApiError, uploadThreadBlob, type UploadObserver } from '../../api/client';
import { awaitThreadStarted, isThreadStartPending, noteServerDraft } from './compose';
import {
  createMemoryPendingUploadBackend,
  ownedPendingUploadRecords,
  readPendingUploadStore,
  _resetPendingUploadRecordsForTesting,
  type PendingUploadBackend,
} from '../pendingUploadRecords';
import {
  addPendingUpload,
  getPendingUpload,
  pendingUploads,
  _resetPendingUploadsForTesting,
  type PendingUploadState,
} from '../pendingUploads';

const pendingUploadsMap = () => pendingUploads.value;
import { connectionStatus, threadMap, toasts } from '../store';
import { getDraft, patchDraft, _resetComposeDraftsForTesting } from '../composeDrafts';
import { noteImageLanded, _resetLandedImagesForTesting } from '../landedImages';
import { _resetSessionBlobUrlsForTesting } from '../../components/chat/pastedImages';
import { makeOptimisticThreadState, type ThreadState } from '../thread-events';

const THREAD = 't-1';
const transportDrop = () => new TypeError('Failed to fetch');
const refusal = (code: number, reason: string) => new ApiError(code, reason);

function addEntry(localId = 'u-1', contentHash?: string): void {
  addPendingUpload({
    localId,
    threadId: THREAD,
    previewUrl: `blob:${localId}`,
    mime: 'image/png',
    state: { kind: 'uploading', sentBytes: 0, totalBytes: 3 },
    file: new File([new Uint8Array([1, 2, 3])], 'a.png', { type: 'image/png' }),
    contentHash,
  });
}

/** SHA-256 of the bytes [1, 2, 3], as the engine's `compute_hash` names them. */
const HASH_123 = '039058c6f2c0cb492c533b0a4d14ef77cc0f78abccced5287d84a1a2011cfb81';

function stateOf(localId = 'u-1'): PendingUploadState | undefined {
  return getPendingUpload(THREAD, localId)?.state;
}

function seedThread(): void {
  const thread: ThreadState = makeOptimisticThreadState({
    id: THREAD, title: '', channel: 'chat', initiator: 'user',
    eventsLoaded: true, state: 'composing', status: 'idle',
  });
  threadMap.value = new Map([[THREAD, thread]]);
}

describe('stateAfterFailure', () => {
  it('fails a refusal at once, and offers no retry for it', () => {
    expect(stateAfterFailure(refusal(415, 'That file is a PDF'), 1, true))
      .toEqual({ kind: 'failed', reason: 'That file is a PDF', retryable: false });
  });

  it('fails an engine error at once, but lets the user retry it', () => {
    expect(stateAfterFailure(refusal(500, 'disk full'), 1, true))
      .toEqual({ kind: 'failed', reason: 'disk full', retryable: true });
  });

  it('retries a transient failure within the budget', () => {
    expect(stateAfterFailure(transportDrop(), 1, true))
      .toEqual({ kind: 'retrying', attempt: 2, reason: 'Lucidos could not be reached' });
    const bootSplash = new ApiError(503, 'Lucidos is restarting', undefined, true);
    expect(stateAfterFailure(bootSplash, 2, true).kind).toBe('retrying');
  });

  it('fails, retryable, once the budget is spent', () => {
    expect(stateAfterFailure(transportDrop(), UPLOAD_RETRY_DELAYS_MS.length + 1, true))
      .toEqual({ kind: 'failed', reason: 'Lucidos could not be reached', retryable: true });
  });

  it('waits for the connection instead of spending the budget while offline', () => {
    expect(stateAfterFailure(transportDrop(), 1, false))
      .toEqual({ kind: 'offline', reason: 'Lucidos could not be reached' });
  });
});

describe('runPendingUpload', () => {
  const originalFetch = globalThis.fetch;
  beforeEach(() => {
    vi.useFakeTimers();
    // The draft PUT that follows a landed hash.
    globalThis.fetch = vi.fn(async () => new Response(null, { status: 204 })) as unknown as typeof fetch;
    connectionStatus.value = 'connected';
    seedThread();
    toasts.value = [];
    vi.mocked(uploadThreadBlob).mockReset();
    vi.mocked(awaitThreadStarted).mockReset().mockImplementation(async () => {});
    vi.mocked(isThreadStartPending).mockReset().mockReturnValue(false);
  });
  afterEach(() => {
    vi.useRealTimers();
    globalThis.fetch = originalFetch;
    _resetImageUploadRunsForTesting();
    _resetPendingUploadsForTesting();
    _resetComposeDraftsForTesting();
    _resetSessionBlobUrlsForTesting();
    threadMap.value = new Map();
    connectionStatus.value = 'disconnected';
  });

  it('walks uploading, finishing, then lands the hash in the draft', async () => {
    const seen: PendingUploadState[] = [];
    vi.mocked(uploadThreadBlob).mockImplementation(async (_t, _f, observer?: UploadObserver) => {
      observer?.onProgress?.({ sentBytes: 1, totalBytes: 3 });
      seen.push(stateOf()!);
      observer?.onBodySent?.();
      seen.push(stateOf()!);
      return { hash: 'h1', mime: 'image/png', byte_size: 3 };
    });
    addEntry();
    await runPendingUpload(THREAD, 'u-1');

    expect(seen).toEqual([
      { kind: 'uploading', sentBytes: 1, totalBytes: 3 },
      { kind: 'finishing' },
    ]);
    expect(stateOf()).toBeUndefined();
    expect(getDraft(THREAD).image_hashes).toEqual(['h1']);
  });

  it('shows the wait for the draft thread while its start is pending', async () => {
    vi.mocked(isThreadStartPending).mockReturnValue(true);
    let release!: () => void;
    vi.mocked(awaitThreadStarted).mockImplementation(() => new Promise<void>((r) => { release = r; }));
    vi.mocked(uploadThreadBlob).mockResolvedValue({ hash: 'h1', mime: 'image/png', byte_size: 3 });
    addEntry();
    const run = runPendingUpload(THREAD, 'u-1');
    await vi.advanceTimersByTimeAsync(0);
    expect(stateOf()).toEqual({ kind: 'waiting-for-thread' });
    release();
    await run;
    expect(getDraft(THREAD).image_hashes).toEqual(['h1']);
  });

  it('retries transient failures on its own, then lands', async () => {
    vi.mocked(uploadThreadBlob)
      .mockRejectedValueOnce(transportDrop())
      .mockRejectedValueOnce(transportDrop())
      .mockResolvedValueOnce({ hash: 'h1', mime: 'image/png', byte_size: 3 });
    addEntry();
    const run = runPendingUpload(THREAD, 'u-1');
    await vi.advanceTimersByTimeAsync(0);
    expect(stateOf()).toEqual({ kind: 'retrying', attempt: 2, reason: 'Lucidos could not be reached' });
    await vi.runAllTimersAsync();
    await run;

    expect(uploadThreadBlob).toHaveBeenCalledTimes(3);
    expect(getDraft(THREAD).image_hashes).toEqual(['h1']);
    expect(toasts.value.filter((t) => t.type === 'error')).toEqual([]);
  });

  it('never stays in flight: a spent budget fails with Retry, and Retry lands it', async () => {
    vi.mocked(uploadThreadBlob).mockRejectedValue(transportDrop());
    addEntry();
    const run = runPendingUpload(THREAD, 'u-1');
    await vi.runAllTimersAsync();
    await run;

    expect(uploadThreadBlob).toHaveBeenCalledTimes(UPLOAD_RETRY_DELAYS_MS.length + 1);
    expect(stateOf()).toEqual({ kind: 'failed', reason: 'Lucidos could not be reached', retryable: true });
    expect(toasts.value.some((t) => t.message.includes('Image upload failed'))).toBe(true);
    // The image is still there for the user to retry or remove.
    expect(getPendingUpload(THREAD, 'u-1')?.file).toBeTruthy();

    vi.mocked(uploadThreadBlob).mockReset().mockResolvedValue({ hash: 'h1', mime: 'image/png', byte_size: 3 });
    await retryPendingUpload(THREAD, 'u-1');
    expect(getDraft(THREAD).image_hashes).toEqual(['h1']);
  });

  it('fails a refusal at once, with no automatic retry', async () => {
    vi.mocked(uploadThreadBlob).mockRejectedValue(refusal(415, 'That file is a PDF'));
    addEntry();
    await runPendingUpload(THREAD, 'u-1');
    expect(uploadThreadBlob).toHaveBeenCalledTimes(1);
    expect(stateOf()).toEqual({ kind: 'failed', reason: 'That file is a PDF', retryable: false });
    await retryPendingUpload(THREAD, 'u-1');
    expect(uploadThreadBlob).toHaveBeenCalledTimes(1);
  });

  it('waits offline for the connection, and resumes on reconnect', async () => {
    connectionStatus.value = 'disconnected';
    vi.mocked(uploadThreadBlob).mockRejectedValueOnce(transportDrop());
    addEntry();
    await runPendingUpload(THREAD, 'u-1');
    expect(stateOf()).toEqual({ kind: 'offline', reason: 'Lucidos could not be reached' });
    await vi.runAllTimersAsync();
    expect(uploadThreadBlob).toHaveBeenCalledTimes(1);

    vi.mocked(uploadThreadBlob).mockResolvedValueOnce({ hash: 'h1', mime: 'image/png', byte_size: 3 });
    connectionStatus.value = 'connected';
    await vi.runAllTimersAsync();
    expect(getDraft(THREAD).image_hashes).toEqual(['h1']);
  });

  it('a cancel aborts the request and never attaches the image', async () => {
    let signal: AbortSignal | undefined;
    let answer!: (v: { hash: string; mime: string; byte_size: number }) => void;
    vi.mocked(uploadThreadBlob).mockImplementation((_t, _f, observer?: UploadObserver) => {
      signal = observer?.signal;
      return new Promise((r) => { answer = r; });
    });
    addEntry();
    const run = runPendingUpload(THREAD, 'u-1');
    await vi.advanceTimersByTimeAsync(0);

    cancelPendingUpload(THREAD, 'u-1');
    expect(signal?.aborted).toBe(true);
    // The answer races the cancel and loses.
    answer({ hash: 'h1', mime: 'image/png', byte_size: 3 });
    await run;
    expect(getDraft(THREAD).image_hashes).toEqual([]);
    expect(stateOf()).toBeUndefined();
  });

  it('drops the chip quietly when its draft is gone', async () => {
    vi.mocked(uploadThreadBlob).mockImplementation(async () => {
      threadMap.value = new Map();
      throw refusal(404, 'thread not found');
    });
    addEntry();
    await runPendingUpload(THREAD, 'u-1');
    expect(stateOf()).toBeUndefined();
    expect(toasts.value.filter((t) => t.type === 'error')).toEqual([]);
  });
});

describe('landing on the ImageUploaded event', () => {
  const originalFetch = globalThis.fetch;

  /** An upload the engine took but whose answer never reaches the page. It
   *  rejects on abort, as `postWithUploadProgress` does. `answer` lands the
   *  late answer anyway, for the race where it loses to the event. */
  function lostAnswer(): { answer: (hash: string) => void; signal: () => AbortSignal | undefined } {
    let answer!: (v: { hash: string; mime: string; byte_size: number }) => void;
    let signal: AbortSignal | undefined;
    vi.mocked(uploadThreadBlob).mockImplementation((_t, _f, observer?: UploadObserver) => {
      signal = observer?.signal;
      return new Promise((resolve, reject) => {
        answer = resolve;
        signal?.addEventListener('abort', () => reject(new DOMException('Upload cancelled', 'AbortError')));
      });
    });
    return { answer: (hash) => answer({ hash, mime: 'image/png', byte_size: 3 }), signal: () => signal };
  }

  beforeEach(() => {
    vi.useFakeTimers();
    globalThis.fetch = vi.fn(async () => new Response(null, { status: 204 })) as unknown as typeof fetch;
    connectionStatus.value = 'connected';
    seedThread();
    toasts.value = [];
    _resetPendingUploadRecordsForTesting();
    vi.mocked(uploadThreadBlob).mockReset();
    vi.mocked(awaitThreadStarted).mockReset().mockImplementation(async () => {});
    vi.mocked(isThreadStartPending).mockReset().mockReturnValue(false);
  });
  afterEach(() => {
    vi.useRealTimers();
    vi.unstubAllGlobals();
    _resetLandedImagesForTesting();
    globalThis.fetch = originalFetch;
    _resetImageUploadRunsForTesting();
    _resetPendingUploadsForTesting();
    _resetComposeDraftsForTesting();
    _resetSessionBlobUrlsForTesting();
    threadMap.value = new Map();
    connectionStatus.value = 'disconnected';
  });

  it('lands an upload whose answer never comes, and uploads nothing again', async () => {
    const upload = lostAnswer();
    addEntry('u-1', 'h1');
    const run = runPendingUpload(THREAD, 'u-1');
    await vi.advanceTimersByTimeAsync(0);

    noteImageLanded(THREAD, 'h1');
    expect(getDraft(THREAD).image_hashes).toEqual(['h1']);
    expect(stateOf()).toBeUndefined();
    expect(upload.signal()?.aborted).toBe(true);

    await run;
    await vi.runAllTimersAsync();
    expect(uploadThreadBlob).toHaveBeenCalledTimes(1);
    expect(toasts.value).toEqual([]);
  });

  it('a late answer after the event changes nothing', async () => {
    const upload = lostAnswer();
    addEntry('u-1', 'h1');
    const run = runPendingUpload(THREAD, 'u-1');
    await vi.advanceTimersByTimeAsync(0);
    noteImageLanded(THREAD, 'h1');
    upload.answer('h1');
    await run;

    expect(getDraft(THREAD).image_hashes).toEqual(['h1']);
    expect(toasts.value).toEqual([]);
  });

  it('the event after the answer changes nothing', async () => {
    vi.mocked(uploadThreadBlob).mockResolvedValue({ hash: 'h1', mime: 'image/png', byte_size: 3 });
    addEntry('u-1', 'h1');
    await runPendingUpload(THREAD, 'u-1');
    noteImageLanded(THREAD, 'h1');

    expect(getDraft(THREAD).image_hashes).toEqual(['h1']);
    expect(toasts.value).toEqual([]);
  });

  it('lands nothing for another image, or for the same image on another draft', async () => {
    const upload = lostAnswer();
    addEntry('u-1', 'h1');
    void runPendingUpload(THREAD, 'u-1');
    await vi.advanceTimersByTimeAsync(0);

    noteImageLanded(THREAD, 'h2');
    noteImageLanded('t-other', 'h1');
    expect(stateOf()?.kind).toBe('uploading');
    expect(getDraft(THREAD).image_hashes).toEqual([]);
    expect(upload.signal()?.aborted).toBe(false);
  });

  it('a cancelled image is not brought back by its event', async () => {
    lostAnswer();
    addEntry('u-1', 'h1');
    const run = runPendingUpload(THREAD, 'u-1');
    await vi.advanceTimersByTimeAsync(0);
    cancelPendingUpload(THREAD, 'u-1');
    noteImageLanded(THREAD, 'h1');
    await run;

    expect(getDraft(THREAD).image_hashes).toEqual([]);
  });

  it('an attached image is named by the hash the engine will give it', async () => {
    lostAnswer();
    const bytes = new Uint8Array([1, 2, 3]).buffer;
    void attachPendingUpload({ threadId: THREAD, file: new File([bytes], 'a.png', { type: 'image/png' }), bytes });
    await vi.waitFor(() => expect(pendingUploadsMap().get(THREAD)?.[0]?.contentHash).toBe(HASH_123));

    noteImageLanded(THREAD, HASH_123);
    expect(getDraft(THREAD).image_hashes).toEqual([HASH_123]);
  });

  it('an event that beats the hash still lands the image once it is hashed', async () => {
    const upload = lostAnswer();
    noteImageLanded(THREAD, HASH_123);
    const bytes = new Uint8Array([1, 2, 3]).buffer;
    const run = attachPendingUpload({ threadId: THREAD, file: new File([bytes], 'a.png', { type: 'image/png' }), bytes });

    await vi.waitFor(() => expect(getDraft(THREAD).image_hashes).toEqual([HASH_123]));
    await run;
    expect(pendingUploadsMap().get(THREAD)).toBeUndefined();
    expect(upload.signal()?.aborted ?? true).toBe(true);
  });

  it('a resumed image lands on its event too', async () => {
    const upload = lostAnswer();
    const run = resumePendingUpload(THREAD, 'u-9', new File([new Uint8Array([1, 2, 3])], 'a.png', { type: 'image/png' }));
    await vi.waitFor(() => expect(upload.signal()).toBeDefined());

    noteImageLanded(THREAD, HASH_123);
    await run;
    expect(getDraft(THREAD).image_hashes).toEqual([HASH_123]);
    expect(uploadThreadBlob).toHaveBeenCalledTimes(1);
  });

  it('without Web Crypto an image carries no hash and still lands on its answer', async () => {
    // What an insecure context keeps: no `subtle`, no `randomUUID`.
    const real = globalThis.crypto;
    vi.stubGlobal('crypto', { getRandomValues: real.getRandomValues.bind(real) });
    vi.mocked(uploadThreadBlob).mockResolvedValue({ hash: 'h1', mime: 'image/png', byte_size: 3 });
    const bytes = new Uint8Array([1, 2, 3]).buffer;
    await attachPendingUpload({ threadId: THREAD, file: new File([bytes], 'a.png', { type: 'image/png' }), bytes });

    expect(getDraft(THREAD).image_hashes).toEqual(['h1']);
  });
});

describe('recordStillNeeded', () => {
  const record = (landedHash?: string) => ({ localId: 'u-1', threadId: THREAD, size: 3, landedHash });

  it('keeps a record while its entry is pending', () => {
    expect(recordStillNeeded(record(), true, [], undefined)).toBe(true);
  });

  it('drops a record whose entry ended without landing', () => {
    expect(recordStillNeeded(record(), false, [], undefined)).toBe(false);
  });

  it('keeps a landed record until the engine holds the hash', () => {
    expect(recordStillNeeded(record('h1'), false, ['h1'], undefined)).toBe(true);
    expect(recordStillNeeded(record('h1'), false, ['h1'], [])).toBe(true);
    expect(recordStillNeeded(record('h1'), false, ['h1'], ['h1'])).toBe(false);
  });

  it('drops a landed record the local draft no longer holds', () => {
    expect(recordStillNeeded(record('h1'), false, [], undefined)).toBe(false);
  });
});

describe('pending upload records', () => {
  const originalFetch = globalThis.fetch;
  const pngBytes = () => new Uint8Array([1, 2, 3]).buffer;
  const attach = () => {
    const bytes = pngBytes();
    return attachPendingUpload({ threadId: THREAD, file: new File([bytes], 'a.png', { type: 'image/png' }), bytes });
  };
  const storedIds = async () => (await readPendingUploadStore()).uploads.map((r) => r.localId);
  const onlyEntryId = () => ownedPendingUploadRecords()[0]?.localId;
  /** The upload leaves once the image is hashed, which is real async work. */
  const uploadStarted = () => vi.waitFor(() => expect(uploadThreadBlob).toHaveBeenCalled());

  /** An upload that waits until the test answers it. */
  function heldUpload(): { answer: (hash: string) => void; signal: () => AbortSignal | undefined } {
    let answer!: (v: { hash: string; mime: string; byte_size: number }) => void;
    let signal: AbortSignal | undefined;
    vi.mocked(uploadThreadBlob).mockImplementation((_t, _f, observer?: UploadObserver) => {
      signal = observer?.signal;
      return new Promise((r) => { answer = r; });
    });
    return { answer: (hash) => answer({ hash, mime: 'image/png', byte_size: 3 }), signal: () => signal };
  }

  beforeEach(() => {
    globalThis.fetch = vi.fn(async () => new Response(null, { status: 204 })) as unknown as typeof fetch;
    connectionStatus.value = 'connected';
    seedThread();
    toasts.value = [];
    _resetPendingUploadRecordsForTesting();
    // An earlier test's draft write left the engine holding a hash.
    noteServerDraft(THREAD, '', []);
    vi.mocked(uploadThreadBlob).mockReset();
    vi.mocked(awaitThreadStarted).mockReset().mockImplementation(async () => {});
    vi.mocked(isThreadStartPending).mockReset().mockReturnValue(false);
  });
  afterEach(() => {
    globalThis.fetch = originalFetch;
    _resetImageUploadRunsForTesting();
    _resetPendingUploadsForTesting();
    _resetComposeDraftsForTesting();
    _resetSessionBlobUrlsForTesting();
    threadMap.value = new Map();
    connectionStatus.value = 'disconnected';
  });

  it('keeps the bytes and metadata the moment an image is attached', async () => {
    heldUpload();
    void attach();
    const { uploads } = await readPendingUploadStore();
    expect(uploads).toHaveLength(1);
    expect(uploads[0]).toMatchObject({ threadId: THREAD, name: 'a.png', mime: 'image/png', size: 3 });
    expect(uploads[0].localId).toBe(getPendingUploadIds()[0]);
  });

  it('the X deletes the record and aborts the request', async () => {
    const upload = heldUpload();
    void attach();
    await uploadStarted();
    const localId = onlyEntryId();
    cancelPendingUpload(THREAD, localId);
    expect(upload.signal()?.aborted).toBe(true);
    expect(await storedIds()).toEqual([]);
  });

  it('a landed image stays on the device until the engine draft holds it', async () => {
    // The draft write that would carry the hash has not been answered yet.
    globalThis.fetch = vi.fn(() => new Promise<Response>(() => {})) as unknown as typeof fetch;
    const upload = heldUpload();
    const run = attach();
    await uploadStarted();
    const localId = onlyEntryId();
    upload.answer('h1');
    await run;

    expect(getDraft(THREAD).image_hashes).toEqual(['h1']);
    expect((await readPendingUploadStore()).uploads[0]).toMatchObject({ localId, landedHash: 'h1' });

    noteServerDraft(THREAD, '', ['h1']);
    expect(await storedIds()).toEqual([]);
  });

  it('a landed image the user removes from the draft is released', async () => {
    const upload = heldUpload();
    const run = attach();
    await uploadStarted();
    upload.answer('h1');
    await run;
    patchDraft(THREAD, { image_hashes: [] });
    expect(await storedIds()).toEqual([]);
  });

  it('a discarded draft cancels its uploads and deletes their records', async () => {
    const upload = heldUpload();
    void attach();
    await uploadStarted();
    const thread = threadMap.value.get(THREAD)!;
    threadMap.value = new Map([[THREAD, { ...thread, meta: { ...thread.meta, state: 'discarded' } }]]);
    expect(upload.signal()?.aborted).toBe(true);
    expect(getPendingUploadIds()).toEqual([]);
    expect(await storedIds()).toEqual([]);
  });

  it('a thread row that goes away takes its uploads with it', async () => {
    heldUpload();
    void attach();
    await uploadStarted();
    threadMap.value = new Map();
    expect(getPendingUploadIds()).toEqual([]);
    expect(await storedIds()).toEqual([]);
  });

  it('a failed upload keeps its record, so a reload can still retry it', async () => {
    vi.mocked(uploadThreadBlob).mockRejectedValue(refusal(415, 'That file is a PDF'));
    await attach();
    expect(await storedIds()).toHaveLength(1);
  });

  it('a device that cannot store still uploads, and says so once', async () => {
    const failing: PendingUploadBackend = {
      ...createMemoryPendingUploadBackend(),
      putUpload: async () => { throw new DOMException('The quota has been exceeded.', 'QuotaExceededError'); },
    };
    _resetPendingUploadRecordsForTesting(failing);
    vi.mocked(uploadThreadBlob).mockResolvedValue({ hash: 'h1', mime: 'image/png', byte_size: 3 });
    await attach();
    await readPendingUploadStore();
    expect(getDraft(THREAD).image_hashes).toEqual(['h1']);
    expect(toasts.value.filter((t) => t.type === 'warning')).toHaveLength(1);
  });
});

function getPendingUploadIds(): string[] {
  return [...(pendingUploadsMap().get(THREAD) ?? [])].map((u) => u.localId);
}
