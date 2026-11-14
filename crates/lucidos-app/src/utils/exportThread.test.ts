import { describe, it, expect, beforeEach, vi } from 'vitest';
import type { ToastAction } from '../store/types';

// Every collaborator is stubbed: the point of this suite is which side of the
// Tauri branch runs, and what the resulting toast offers.
const platformMocks = vi.hoisted(() => ({ isTauri: false, isIOS: false }));
vi.mock('./platform', () => ({
  isTauri: () => platformMocks.isTauri,
  isIOS: () => platformMocks.isIOS,
}));

const saveToDownloads = vi.hoisted(() => vi.fn());
vi.mock('./tauri', () => ({ saveToDownloads }));

const openLocalFile = vi.hoisted(() => vi.fn());
vi.mock('../store/actions/artifacts', () => ({ openLocalFile }));

const fetchThreadEvents = vi.hoisted(() => vi.fn());
vi.mock('../api/threads', () => ({ fetchThreadEvents }));

const storeMocks = vi.hoisted(() => ({
  showToast: vi.fn(),
  workspaceName: { value: 'dev' },
}));
vi.mock('../store/store', () => storeMocks);

const { exportThread } = await import('./exportThread');

const THREAD_ID = '1a2b3c4d-0000-0000-0000-000000000000';
const FILENAME = 'thread-1a2b3c4d-my-thread.json';

interface FakeAnchor {
  href: string;
  download: string;
  click: ReturnType<typeof vi.fn>;
  remove: () => void;
}

/** The anchors `triggerDownload` built this test. The node env has no real DOM,
 *  so the download is observed through a `document` stub. */
let anchors: FakeAnchor[];

/** Let a rejected promise's `.catch` run. The share handler is deliberately not
 *  awaitable from the caller, so its toast lands a tick later. */
function flushMicrotasks(): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, 0));
}

/** The most recent toast, as (message, type, action). */
function lastToast(): { message: string; type: string; action: ToastAction | undefined } {
  const calls = storeMocks.showToast.mock.calls;
  const call = calls[calls.length - 1];
  return { message: call[0], type: call[1], action: call[2]?.action };
}

/** Install a `navigator` that accepts (or refuses) a file share. */
function stubShare(opts: { canShare: boolean; share?: () => Promise<void> }): ReturnType<typeof vi.fn> {
  const share = vi.fn(opts.share ?? (() => Promise.resolve()));
  vi.stubGlobal('navigator', {
    canShare: (data: { files?: File[] }) => opts.canShare && !!data.files?.length,
    share,
  });
  return share;
}

describe('exportThread', () => {
  beforeEach(() => {
    platformMocks.isTauri = false;
    platformMocks.isIOS = false;
    saveToDownloads.mockReset();
    openLocalFile.mockReset();
    storeMocks.showToast.mockClear();
    fetchThreadEvents.mockReset();
    fetchThreadEvents.mockResolvedValue({ events: [], currentAggregate: null });

    anchors = [];
    vi.stubGlobal('document', {
      createElement: (tag: string) => {
        const el: FakeAnchor = { href: '', download: '', click: vi.fn(), remove: () => {} };
        if (tag === 'a') anchors.push(el);
        return el;
      },
      body: { appendChild: () => {} },
    });
    vi.stubGlobal('URL', { createObjectURL: () => 'blob:x', revokeObjectURL: () => {} });
    // No share support unless a case asks for it.
    vi.stubGlobal('navigator', {});
  });

  describe('on the desktop app', () => {
    beforeEach(() => {
      platformMocks.isTauri = true;
      saveToDownloads.mockResolvedValue({
        dir: '/Users/me/Downloads',
        path: `/Users/me/Downloads/${FILENAME}`,
      });
    });

    it('saves through the command instead of the dead webview download', async () => {
      await exportThread(THREAD_ID, 'My thread');

      expect(saveToDownloads).toHaveBeenCalledOnce();
      const [filename, contents] = saveToDownloads.mock.calls[0];
      expect(filename).toBe(FILENAME);
      expect(JSON.parse(contents)).toMatchObject({ thread_id: THREAD_ID, title: 'My thread' });
      expect(anchors).toHaveLength(0);
    });

    /** The folder in the toast is the one the command reported, never a guess. */
    it('names the folder it was told about, and opens that same folder', async () => {
      saveToDownloads.mockResolvedValue({ dir: '/Users/me/Elsewhere', path: '/x' });

      await exportThread(THREAD_ID, 'My thread');

      const toast = lastToast();
      expect(toast.message).toBe('Thread exported to Elsewhere');
      expect(toast.type).toBe('success');
      expect(toast.action?.label).toBe('Open folder');
      toast.action?.onClick();
      expect(openLocalFile).toHaveBeenCalledWith('/Users/me/Elsewhere');
    });

    it('surfaces a failed save with its reason', async () => {
      saveToDownloads.mockRejectedValue('could not write /Users/me/Downloads/x: denied');

      await exportThread(THREAD_ID, 'My thread');

      const toast = lastToast();
      expect(toast.type).toBe('error');
      expect(toast.message).toContain('denied');
    });
  });

  describe('in a browser', () => {
    it('keeps the blob download and names the downloads folder', async () => {
      await exportThread(THREAD_ID, 'My thread');

      expect(saveToDownloads).not.toHaveBeenCalled();
      expect(anchors).toHaveLength(1);
      expect(anchors[0].download).toBe(FILENAME);
      expect(anchors[0].click).toHaveBeenCalledOnce();
      expect(lastToast().message).toBe('Thread exported to your downloads folder');
    });

    /** No web API opens a folder, so an `Open folder` button here would be dead. */
    it('offers no action where the platform cannot share', async () => {
      await exportThread(THREAD_ID, 'My thread');

      expect(lastToast().action).toBeUndefined();
    });

    it('offers Share where the platform can take the file, download included', async () => {
      const share = stubShare({ canShare: true });

      await exportThread(THREAD_ID, 'My thread');

      expect(anchors[0].click).toHaveBeenCalledOnce();
      const toast = lastToast();
      expect(toast.action?.label).toBe('Share');
      toast.action?.onClick();
      expect(share).toHaveBeenCalledOnce();
      const shared = share.mock.calls[0][0] as { files: File[] };
      expect(shared.files[0].name).toBe(FILENAME);
    });

    it('says nothing when the user dismisses the share sheet', async () => {
      stubShare({
        canShare: true,
        share: () => Promise.reject(new DOMException('cancelled', 'AbortError')),
      });

      await exportThread(THREAD_ID, 'My thread');
      const toastCount = storeMocks.showToast.mock.calls.length;
      lastToast().action?.onClick();
      await flushMicrotasks();

      expect(storeMocks.showToast.mock.calls).toHaveLength(toastCount);
    });

    it('surfaces a share that fails for any other reason', async () => {
      stubShare({ canShare: true, share: () => Promise.reject(new Error('no target')) });

      await exportThread(THREAD_ID, 'My thread');
      lastToast().action?.onClick();
      await flushMicrotasks();

      const toast = lastToast();
      expect(toast.type).toBe('error');
      expect(toast.message).toContain('no target');
    });

    it('surfaces a failed snapshot fetch', async () => {
      fetchThreadEvents.mockRejectedValue(new Error('thread is gone'));

      await exportThread(THREAD_ID, 'My thread');

      const toast = lastToast();
      expect(toast.type).toBe('error');
      expect(toast.message).toContain('thread is gone');
      expect(anchors).toHaveLength(0);
    });
  });

  describe('on iOS', () => {
    beforeEach(() => {
      platformMocks.isIOS = true;
    });

    /** The regression: the anchor opened the JSON in a viewer, taking the PWA
     *  off the thread, and only then did the toast arrive. */
    it('offers the share sheet alone, never the anchor that opens the JSON', async () => {
      const share = stubShare({ canShare: true });

      await exportThread(THREAD_ID, 'My thread');

      expect(anchors).toHaveLength(0);
      const toast = lastToast();
      expect(toast.message).toBe('Thread export ready');
      expect(toast.type).toBe('success');
      expect(toast.action?.label).toBe('Share');
      toast.action?.onClick();
      expect(share).toHaveBeenCalledOnce();
      const shared = share.mock.calls[0][0] as { files: File[] };
      expect(shared.files[0].name).toBe(FILENAME);
    });

    /** No share sheet means the anchor is the only route left, poor as it is. */
    it('keeps the download where the share sheet refuses the file', async () => {
      stubShare({ canShare: false });

      await exportThread(THREAD_ID, 'My thread');

      expect(anchors).toHaveLength(1);
      expect(anchors[0].click).toHaveBeenCalledOnce();
      expect(lastToast().message).toBe('Thread exported to your downloads folder');
      expect(lastToast().action).toBeUndefined();
    });
  });
});
