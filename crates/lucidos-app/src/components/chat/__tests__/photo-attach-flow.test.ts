/**
 * End-to-end behaviour test: tapping the photo button → picker dismisses with
 * a selected file → handleFileSelect runs → image lands in the draft preview
 * strip (`pastedImagesForCurrentThread`).
 *
 * The previous regression test (photo-attach-ios-pwa.test.ts) only static-
 * checked CSS class names on the file input. It failed to catch the actual
 * data-flow bug, so this test exercises the chain that the iOS PWA hits.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { connectionStatus, focusedThreadId, threadMap } from '../../../store/store';
import type { ThreadMeta, ThreadState } from '../../../store/thread-events';
import { attachImageToActiveDraft } from '../attachToDraft';
import { pastedImagesForCurrentThread } from '../pastedImages';
import { _resetComposeDraftsForTesting, getDraft } from '../../../store/composeDrafts';

const originalFetch = globalThis.fetch;
const originalFileReader = (globalThis as any).FileReader;

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
      ccHasChanges: false,
      ccRequiresRestart: false,
      ccIsExternalRepo: false,
      ccApplying: false,
      lastRevivedAt: '',
      messageCount: 0,
      section: 'archived',
      activeChildrenCount: 0,
      totalChildrenCount: 0,
      state: 'active',
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

/** Bypass the real async FileReader — resolve synchronously with a fixed
 *  data URL. The image bytes don't matter; the test asserts that whatever
 *  the reader resolved with reaches the threadMap. */
function installFakeFileReader(dataUrl: string) {
  class FakeFileReader {
    onload: ((this: FileReader, ev: ProgressEvent<FileReader>) => void) | null = null;
    onerror: ((this: FileReader, ev: ProgressEvent<FileReader>) => void) | null = null;
    error: DOMException | null = null;
    result: string | ArrayBuffer | null = null;
    readAsDataURL(_file: Blob): void {
      this.result = dataUrl;
      queueMicrotask(() => { this.onload?.call(this as unknown as FileReader, {} as ProgressEvent<FileReader>); });
    }
  }
  (globalThis as any).FileReader = FakeFileReader;
}

function fakeFile(): File {
  return new Blob([new Uint8Array([0xff, 0xd8, 0xff])], { type: 'image/jpeg' }) as unknown as File;
}

describe('photo attach reaches the draft preview', () => {
  let mockFetch: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    mockFetch = vi.fn().mockResolvedValue(new Response(null, { status: 200 }));
    globalThis.fetch = mockFetch as unknown as typeof fetch;
    connectionStatus.value = 'connected';
    focusedThreadId.value = null;
    threadMap.value = new Map();
    installFakeFileReader('data:image/jpeg;base64,/9j/AAQ=');
  });

  afterEach(() => {
    globalThis.fetch = originalFetch;
    (globalThis as any).FileReader = originalFileReader;
    connectionStatus.value = 'disconnected';
    focusedThreadId.value = null;
    threadMap.value = new Map();
    _resetComposeDraftsForTesting();
    vi.restoreAllMocks();
  });

  it('attaches to a focused active thread and surfaces in the preview strip', async () => {
    focusedThreadId.value = 't-active';
    threadMap.value = new Map([['t-active', makeActiveThread()]]);

    await attachImageToActiveDraft(fakeFile());

    const stripImages = pastedImagesForCurrentThread.value;
    expect(stripImages).toHaveLength(1);
    expect(stripImages[0].mimeType).toBe('image/jpeg');
    expect(getDraft('t-active').images).toHaveLength(1);
  });

  it('attaches in compose view (no focused thread) by lazy-creating a draft', async () => {
    expect(focusedThreadId.value).toBeNull();
    expect(threadMap.value.size).toBe(0);

    await attachImageToActiveDraft(fakeFile());

    const id = focusedThreadId.value;
    expect(id).not.toBeNull();
    const thread = threadMap.value.get(id!);
    expect(thread, 'lazy-created compose thread should be in threadMap').toBeDefined();
    expect(getDraft(id!).images).toHaveLength(1);

    const stripImages = pastedImagesForCurrentThread.value;
    expect(stripImages).toHaveLength(1);
  });
});
