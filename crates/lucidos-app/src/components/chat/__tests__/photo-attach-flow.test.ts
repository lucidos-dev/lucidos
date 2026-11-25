import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { connectionStatus, focusedThreadId, threadMap } from '../../../store/store';
import type { ThreadMeta, ThreadState } from '../../../store/thread-events';
import { attachImageToActiveDraft } from '../attachToDraft';
import {
  attachedImagesForCurrentThread,
  getSessionBlobUrlForHash,
  markHashesAsSent,
  removeAttachedImage,
  _resetSessionBlobUrlsForTesting,
} from '../pastedImages';
import { _resetComposeDraftsForTesting, clearDraft, getDraft } from '../../../store/composeDrafts';
import {
  _resetPendingUploadsForTesting,
  pendingUploads,
} from '../../../store/pendingUploads';
import { installXhrOverFetch, uninstallXhrOverFetch } from '../../../api/client/__tests__/xhrOverFetch';

const originalFetch = globalThis.fetch;
const originalCreateObjectURL = (globalThis as any).URL?.createObjectURL;
const originalRevokeObjectURL = (globalThis as any).URL?.revokeObjectURL;

function makeActiveThread(overrides: Partial<ThreadMeta> = {}): ThreadState {
  return {
    meta: {
      id: 't-active',
      title: '',
      channel: 'chat',
      initiator: 'user',
      saved: false,
      createdAt: '',
      updatedAt: '',
      status: 'idle',
      summaryVersion: 0,
      codingAgentChangeState: { kind: 'none' },
      codingAgentIsExternalRepo: false,
      lastRevivedAt: '',
      messageCount: 0,
      section: 'archived',
      activeChildrenCount: 0,
      totalChildrenCount: 0,
      blockingDescendantCount: 0, attentionDescendantCount: 0, archivableSubThreadCount: 0,
      state: 'active',
      latestTodoList: null,
      liveEventWaitCount: 0,
      liveEventWaits: [],
      ...overrides,
    },
    events: new Map(),
    streamingBuffer: '',
    eventsLoaded: true,
    eventsLoadFailed: false,
    lastDbSeq: 0,
    pendingUserMessages: [],
  };
}

function fakeFile(): File {
  return new Blob([new Uint8Array([0xff, 0xd8, 0xff])], { type: 'image/jpeg' }) as unknown as File;
}

describe('photo attach reaches the draft preview', () => {
  let mockFetch: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    mockFetch = vi.fn(() => Promise.resolve(
      new Response(JSON.stringify({ hash: 'fake-hash-abc123', mime: 'image/jpeg', byte_size: 3 }), {
        status: 200,
        headers: { 'Content-Type': 'application/json' },
      }),
    ));
    globalThis.fetch = mockFetch as unknown as typeof fetch;
    installXhrOverFetch();
    (globalThis as any).URL.createObjectURL = vi.fn().mockReturnValue('blob:fake-preview-url');
    (globalThis as any).URL.revokeObjectURL = vi.fn();
    connectionStatus.value = 'connected';
    focusedThreadId.value = null;
    threadMap.value = new Map();
    _resetComposeDraftsForTesting();
    _resetPendingUploadsForTesting();
    _resetSessionBlobUrlsForTesting();
  });

  afterEach(() => {
    globalThis.fetch = originalFetch;
    uninstallXhrOverFetch();
    if (originalCreateObjectURL) (globalThis as any).URL.createObjectURL = originalCreateObjectURL;
    if (originalRevokeObjectURL) (globalThis as any).URL.revokeObjectURL = originalRevokeObjectURL;
    connectionStatus.value = 'disconnected';
    focusedThreadId.value = null;
    threadMap.value = new Map();
    _resetComposeDraftsForTesting();
    _resetPendingUploadsForTesting();
    _resetSessionBlobUrlsForTesting();
    vi.restoreAllMocks();
  });

  it('attaches to a focused active thread and keeps the blob: URL for the preview', async () => {
    focusedThreadId.value = 't-active';
    threadMap.value = new Map([['t-active', makeActiveThread()]]);

    await attachImageToActiveDraft(fakeFile());

    const stripImages = attachedImagesForCurrentThread.value;
    expect(stripImages).toHaveLength(1);
    expect(stripImages[0].hash).toBe('fake-hash-abc123');
    expect(stripImages[0].previewUrl).toBe('blob:fake-preview-url');
    expect(getDraft('t-active').image_hashes).toEqual(['fake-hash-abc123']);
    expect(pendingUploads.value.get('t-active')).toBeUndefined();
  });

  it('attaches in compose view (no focused thread) by lazy-creating a draft', async () => {
    expect(focusedThreadId.value).toBeNull();
    expect(threadMap.value.size).toBe(0);

    await attachImageToActiveDraft(fakeFile());

    const id = focusedThreadId.value;
    expect(id).not.toBeNull();
    const thread = threadMap.value.get(id!);
    expect(thread, 'lazy-created compose thread should be in threadMap').toBeDefined();
    expect(getDraft(id!).image_hashes).toEqual(['fake-hash-abc123']);

    const stripImages = attachedImagesForCurrentThread.value;
    expect(stripImages).toHaveLength(1);
  });

  it('does not POST the blob until POST /threads resolves (compose race)', async () => {
    expect(focusedThreadId.value).toBeNull();

    const seen: Array<{ method: string; url: string }> = [];
    let resolveCreate: ((r: Response) => void) | null = null;

    mockFetch.mockImplementation((url: string, init?: RequestInit) => {
      const method = (init?.method ?? 'GET').toUpperCase();
      seen.push({ method, url });
      if (url === '/api/v1/threads' && method === 'POST') {
        return new Promise<Response>((r) => { resolveCreate = r; });
      }
      if (url.endsWith('/blobs') && method === 'POST') {
        return Promise.resolve(new Response(
          JSON.stringify({ hash: 'fake-hash', mime: 'image/jpeg', byte_size: 3 }),
          { status: 200, headers: { 'Content-Type': 'application/json' } },
        ));
      }
      return Promise.resolve(new Response(null, { status: 200 }));
    });

    const isBlobPost = (s: { method: string; url: string }) =>
      s.method === 'POST' && s.url.endsWith('/blobs');

    const attachP = attachImageToActiveDraft(fakeFile());

    await new Promise((r) => setTimeout(r, 0));

    expect(
      seen.filter(isBlobPost),
      'blob POST must NOT fire before POST /threads resolves',
    ).toEqual([]);

    resolveCreate!(new Response(null, { status: 200 }));
    await attachP;

    expect(seen.filter(isBlobPost)).toHaveLength(1);
    const id = focusedThreadId.value!;
    expect(getDraft(id).image_hashes).toEqual(['fake-hash']);
  });

  it('does not swap the preview to the server URL after upload (no cold-fetch black flash)', async () => {
    focusedThreadId.value = 't-active';
    threadMap.value = new Map([['t-active', makeActiveThread()]]);

    let imagesConstructed = 0;
    class SpyImage {
      onload: (() => void) | null = null;
      onerror: (() => void) | null = null;
      set src(_: string) { imagesConstructed += 1; }
    }
    const originalImage = (globalThis as { Image?: typeof Image }).Image;
    (globalThis as { Image?: unknown }).Image = SpyImage as unknown;

    try {
      await attachImageToActiveDraft(fakeFile());

      expect(pendingUploads.value.get('t-active')).toBeUndefined();
      const stripImages = attachedImagesForCurrentThread.value;
      expect(stripImages).toHaveLength(1);
      expect(stripImages[0].previewUrl).toBe('blob:fake-preview-url');
      expect(getDraft('t-active').image_hashes).toEqual(['fake-hash-abc123']);
      expect(imagesConstructed).toBe(0);
    } finally {
      if (originalImage === undefined) {
        delete (globalThis as { Image?: unknown }).Image;
      } else {
        (globalThis as { Image?: unknown }).Image = originalImage;
      }
    }
  });

  it('drops the session blob URL when an attached (unsent) image is removed via X', async () => {
    focusedThreadId.value = 't-active';
    threadMap.value = new Map([['t-active', makeActiveThread()]]);

    await attachImageToActiveDraft(fakeFile());
    expect(attachedImagesForCurrentThread.value[0].previewUrl).toBe('blob:fake-preview-url');

    removeAttachedImage('t-active', 0);

    expect((globalThis as any).URL.revokeObjectURL).toHaveBeenCalledWith('blob:fake-preview-url');
    expect(getSessionBlobUrlForHash('fake-hash-abc123')).toBeNull();
  });

  it('keeps the session blob URL alive after send so UserMessageBody renders in-memory bytes (no iOS PWA black flash)', async () => {
    focusedThreadId.value = 't-active';
    threadMap.value = new Map([['t-active', makeActiveThread()]]);

    await attachImageToActiveDraft(fakeFile());
    expect(getSessionBlobUrlForHash('fake-hash-abc123')).toBe('blob:fake-preview-url');

    markHashesAsSent(['fake-hash-abc123']);
    clearDraft('t-active');

    expect((globalThis as any).URL.revokeObjectURL).not.toHaveBeenCalledWith('blob:fake-preview-url');
    expect(getSessionBlobUrlForHash('fake-hash-abc123')).toBe('blob:fake-preview-url');
  });

  it('failed upload leaves a retry chip in pendingUploads', async () => {
    focusedThreadId.value = 't-active';
    threadMap.value = new Map([['t-active', makeActiveThread()]]);
    mockFetch.mockResolvedValueOnce(
      new Response(JSON.stringify({ error: 'unsupported mime' }), {
        status: 415,
        headers: { 'Content-Type': 'application/json' },
      }),
    );

    await attachImageToActiveDraft(fakeFile());

    expect(getDraft('t-active').image_hashes).toEqual([]);
    const pending = pendingUploads.value.get('t-active');
    expect(pending).toHaveLength(1);
    expect(pending![0].state).toEqual({ kind: 'failed', reason: 'unsupported mime', retryable: false });
  });
});
