import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';

// Override Node's URL.createObjectURL/revokeObjectURL — the real ones require a
// Blob instance, but our test uses a plain `File`-shaped object.
(globalThis as any).URL.createObjectURL = () => 'blob:fake';
(globalThis as any).URL.revokeObjectURL = () => {};

// Real `updateCompose` writes through to the real draft signal — exactly what
// the bug needs to expose. We only stub the two thread-bootstrap helpers so
// the test doesn't need a server-side thread row.
vi.mock('../../store/actions/compose', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../../store/actions/compose')>();
  return {
    ...actual,
    ensureFocusedComposeThread: vi.fn(() => 't-1'),
    awaitThreadStarted: vi.fn(async () => {}),
  };
});

vi.mock('../../api/client', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../../api/client')>();
  return {
    ...actual,
    uploadThreadBlob: vi.fn(),
    blobPreviewUrl: (hash: string) => `/api/v1/blobs/${hash}/preview`,
  };
});

import { splitDroppedFiles, attachDroppedFilesToDraft, attachImageToActiveDraft } from './attachToDraft';
import { toasts } from '../../store/store';
import { getDraft, _resetComposeDraftsForTesting } from '../../store/composeDrafts';
import {
  pendingUploads,
  removePendingUpload,
  _resetPendingUploadsForTesting,
} from '../../store/pendingUploads';
import { uploadThreadBlob } from '../../api/client';
import { awaitThreadStarted } from '../../store/actions/compose';

function makeFakeFile(name: string, type: string): File {
  // `attachImageToActiveDraft` snapshots the bytes via `arrayBuffer()` before
  // uploading (see the Universal Clipboard fix), so the fake must expose it.
  return {
    name,
    type,
    arrayBuffer: async () => new Uint8Array([0xff, 0xd8, 0xff]).buffer,
  } as unknown as File;
}

function makeFakeFileList(files: File[]): FileList {
  const list: any = { length: files.length, item: (i: number) => files[i] ?? null };
  files.forEach((f, i) => { list[i] = f; });
  return list as FileList;
}

beforeEach(() => {
  toasts.value = [];
});

describe('splitDroppedFiles', () => {
  it('classifies image MIME types as images', () => {
    const files = makeFakeFileList([
      makeFakeFile('a.png', 'image/png'),
      makeFakeFile('b.jpg', 'image/jpeg'),
      makeFakeFile('c.webp', 'image/webp'),
    ]);
    const split = splitDroppedFiles(files);
    expect(split.images).toHaveLength(3);
    expect(split.plugins).toEqual([]);
    expect(split.skipped).toEqual([]);
  });

  it('skips non-image files and keeps the File objects', () => {
    const doc = makeFakeFile('doc.pdf', 'application/pdf');
    const notes = makeFakeFile('notes.txt', 'text/plain');
    const split = splitDroppedFiles(makeFakeFileList([doc, notes]));
    expect(split.images).toEqual([]);
    expect(split.plugins).toEqual([]);
    expect(split.skipped).toEqual([doc, notes]);
  });

  it('handles a mix of image and non-image files', () => {
    const img = makeFakeFile('photo.png', 'image/png');
    const zip = makeFakeFile('archive.zip', 'application/zip');
    const split = splitDroppedFiles(makeFakeFileList([img, zip]));
    expect(split.images).toEqual([img]);
    expect(split.plugins).toEqual([]);
    expect(split.skipped).toEqual([zip]);
  });

  it('treats files with no MIME type as non-image', () => {
    const mystery = makeFakeFile('mystery', '');
    const split = splitDroppedFiles(makeFakeFileList([mystery]));
    expect(split.images).toEqual([]);
    expect(split.plugins).toEqual([]);
    expect(split.skipped).toEqual([mystery]);
  });

  it('classifies .lucidos-plugin files as plugins regardless of MIME', () => {
    const a = makeFakeFile('no-role-playing-0.1.1.lucidos-plugin', 'application/octet-stream');
    const b = makeFakeFile('Foo.LUCIDOS-PLUGIN', '');
    const split = splitDroppedFiles(makeFakeFileList([a, b]));
    expect(split.plugins).toEqual([a, b]);
    expect(split.images).toEqual([]);
    expect(split.skipped).toEqual([]);
  });

  it('puts plugins in their own bucket alongside images and skipped', () => {
    const img = makeFakeFile('photo.png', 'image/png');
    const plug = makeFakeFile('thing.lucidos-plugin', '');
    const other = makeFakeFile('notes.pdf', 'application/pdf');
    const split = splitDroppedFiles(makeFakeFileList([img, plug, other]));
    expect(split.images).toEqual([img]);
    expect(split.plugins).toEqual([plug]);
    expect(split.skipped).toEqual([other]);
  });
});

describe('attachDroppedFilesToDraft', () => {
  it('offers to import a single non-image file and imports it on confirm', async () => {
    const attachImage = vi.fn().mockResolvedValue(undefined);
    const installPlugin = vi.fn().mockResolvedValue(undefined);
    const importFiles = vi.fn().mockResolvedValue(undefined);
    const confirm = vi.fn().mockResolvedValue(true);
    const pdf = makeFakeFile('report.pdf', 'application/pdf');
    await attachDroppedFilesToDraft(
      makeFakeFileList([pdf]),
      attachImage,
      installPlugin,
      importFiles,
      confirm,
    );
    expect(attachImage).not.toHaveBeenCalled();
    expect(installPlugin).not.toHaveBeenCalled();
    expect(confirm).toHaveBeenCalledOnce();
    expect(confirm.mock.calls[0][0]).toContain('report.pdf');
    expect(importFiles).toHaveBeenCalledWith([pdf]);
    // No warning toast — the confirm dialog replaces it.
    expect(toasts.value).toEqual([]);
  });

  it('does not import when the user cancels the confirm', async () => {
    const importFiles = vi.fn().mockResolvedValue(undefined);
    const confirm = vi.fn().mockResolvedValue(false);
    await attachDroppedFilesToDraft(
      makeFakeFileList([makeFakeFile('report.pdf', 'application/pdf')]),
      vi.fn().mockResolvedValue(undefined),
      vi.fn().mockResolvedValue(undefined),
      importFiles,
      confirm,
    );
    expect(confirm).toHaveBeenCalledOnce();
    expect(importFiles).not.toHaveBeenCalled();
    expect(toasts.value).toEqual([]);
  });

  it('asks once with an aggregated message when multiple non-image files are dropped', async () => {
    const importFiles = vi.fn().mockResolvedValue(undefined);
    const confirm = vi.fn().mockResolvedValue(true);
    const a = makeFakeFile('a.pdf', 'application/pdf');
    const b = makeFakeFile('b.zip', 'application/zip');
    await attachDroppedFilesToDraft(
      makeFakeFileList([a, b]),
      vi.fn().mockResolvedValue(undefined),
      vi.fn().mockResolvedValue(undefined),
      importFiles,
      confirm,
    );
    expect(confirm).toHaveBeenCalledOnce();
    expect(confirm.mock.calls[0][0]).toContain('2 files');
    expect(importFiles).toHaveBeenCalledWith([a, b]);
  });

  it('attaches image files via the supplied attacher', async () => {
    const attachImage = vi.fn().mockResolvedValue(undefined);
    const installPlugin = vi.fn().mockResolvedValue(undefined);
    const a = makeFakeFile('a.png', 'image/png');
    const b = makeFakeFile('b.jpg', 'image/jpeg');
    await attachDroppedFilesToDraft(
      makeFakeFileList([a, b]),
      attachImage,
      installPlugin,
    );
    expect(attachImage).toHaveBeenCalledTimes(2);
    expect(attachImage).toHaveBeenNthCalledWith(1, a);
    expect(attachImage).toHaveBeenNthCalledWith(2, b);
    expect(installPlugin).not.toHaveBeenCalled();
    expect(toasts.value).toEqual([]);
  });

  it('attaches images and offers to import skipped files in a mixed drop', async () => {
    const attachImage = vi.fn().mockResolvedValue(undefined);
    const installPlugin = vi.fn().mockResolvedValue(undefined);
    const importFiles = vi.fn().mockResolvedValue(undefined);
    const confirm = vi.fn().mockResolvedValue(true);
    const img = makeFakeFile('cat.png', 'image/png');
    const pdf = makeFakeFile('notes.pdf', 'application/pdf');
    await attachDroppedFilesToDraft(
      makeFakeFileList([img, pdf]),
      attachImage,
      installPlugin,
      importFiles,
      confirm,
    );
    expect(attachImage).toHaveBeenCalledOnce();
    expect(attachImage).toHaveBeenCalledWith(img);
    expect(installPlugin).not.toHaveBeenCalled();
    expect(confirm).toHaveBeenCalledOnce();
    expect(confirm.mock.calls[0][0]).toContain('notes.pdf');
    expect(importFiles).toHaveBeenCalledWith([pdf]);
  });

  it('routes .lucidos-plugin drops to the install handler instead of skipping', async () => {
    const attachImage = vi.fn().mockResolvedValue(undefined);
    const installPlugin = vi.fn().mockResolvedValue(undefined);
    const importFiles = vi.fn().mockResolvedValue(undefined);
    const confirm = vi.fn().mockResolvedValue(true);
    const plug = makeFakeFile('thing-0.1.0.lucidos-plugin', 'application/octet-stream');
    await attachDroppedFilesToDraft(
      makeFakeFileList([plug]),
      attachImage,
      installPlugin,
      importFiles,
      confirm,
    );
    expect(attachImage).not.toHaveBeenCalled();
    expect(installPlugin).toHaveBeenCalledOnce();
    expect(installPlugin).toHaveBeenCalledWith(plug);
    expect(confirm).not.toHaveBeenCalled();
    expect(importFiles).not.toHaveBeenCalled();
    expect(toasts.value).toEqual([]);
  });

  it('handles a mixed drop of image, plugin, and unsupported file', async () => {
    const attachImage = vi.fn().mockResolvedValue(undefined);
    const installPlugin = vi.fn().mockResolvedValue(undefined);
    const importFiles = vi.fn().mockResolvedValue(undefined);
    const confirm = vi.fn().mockResolvedValue(true);
    const img = makeFakeFile('cat.png', 'image/png');
    const plug = makeFakeFile('p.lucidos-plugin', '');
    const other = makeFakeFile('readme.txt', 'text/plain');
    await attachDroppedFilesToDraft(
      makeFakeFileList([img, plug, other]),
      attachImage,
      installPlugin,
      importFiles,
      confirm,
    );
    expect(attachImage).toHaveBeenCalledWith(img);
    expect(installPlugin).toHaveBeenCalledWith(plug);
    expect(confirm).toHaveBeenCalledOnce();
    expect(confirm.mock.calls[0][0]).toContain('readme.txt');
    expect(importFiles).toHaveBeenCalledWith([other]);
  });
});

/** Cancelling an upload while it is still in flight (clicking the X on the
 *  pending preview) must NOT later add the hash to the draft when the upload
 *  eventually completes. The pending entry's removal IS the user's "cancel"
 *  signal — the in-flight POST resolves regardless, and without a guard the
 *  batched commit re-attaches the image as a confirmed hash. The user sees
 *  the image disappear and pop back a moment later. */
describe('attachImageToActiveDraft respects cancellation mid-upload', () => {
  beforeEach(() => {
    _resetPendingUploadsForTesting();
    _resetComposeDraftsForTesting();
    vi.mocked(uploadThreadBlob).mockReset();
  });

  afterEach(() => {
    _resetPendingUploadsForTesting();
    _resetComposeDraftsForTesting();
  });

  it('does not add the hash when the pending upload was removed before the upload resolved', async () => {
    let resolveUpload!: (value: { hash: string; mime: string; byte_size: number }) => void;
    vi.mocked(uploadThreadBlob).mockImplementation(
      () => new Promise((resolve) => { resolveUpload = resolve; }),
    );

    const file = makeFakeFile('photo.png', 'image/png');
    const attachPromise = attachImageToActiveDraft(file);

    // Flush microtasks so the byte snapshot (`arrayBuffer()`) resolves — the
    // pending preview is added after it — and the awaited `awaitThreadStarted`
    // resolves so `uploadThreadBlob` is invoked (capturing `resolveUpload`).
    await new Promise((r) => setTimeout(r, 0));

    // Pending preview is rendered; user sees the image.
    const pendingForThread = pendingUploads.value.get('t-1') ?? [];
    expect(pendingForThread).toHaveLength(1);
    const localId = pendingForThread[0].localId;
    expect(uploadThreadBlob).toHaveBeenCalledTimes(1);

    // User clicks X on the still-uploading preview to cancel.
    removePendingUpload('t-1', localId);
    expect(pendingUploads.value.get('t-1')).toBeUndefined();
    expect(getDraft('t-1').image_hashes).toEqual([]);

    // Upload completes server-side anyway — the request was already in flight.
    resolveUpload({ hash: 'sha256-of-photo', mime: 'image/png', byte_size: 1 });
    await attachPromise;

    // The image must stay gone. Before the fix, the draft picks up the hash
    // and the strip re-renders the image as a confirmed attachment.
    expect(getDraft('t-1').image_hashes).toEqual([]);
    expect(pendingUploads.value.get('t-1')).toBeUndefined();
  });
});

/** Regression: an image pasted from the system clipboard (notably macOS
 *  Universal Clipboard — copied on an iPhone, pasted on the Mac) is a File
 *  backed by a promised pasteboard resource, readable only during the
 *  synchronous paste-event turn. The old code read its bytes lazily — inside
 *  `uploadThreadBlob`'s FormData serialization, AFTER `await awaitThreadStarted`
 *  — by which point the backing was released and the upload `fetch` threw
 *  `TypeError: Failed to fetch`. The fix snapshots the bytes into an in-memory
 *  File up front, before that await. */
describe('attachImageToActiveDraft snapshots clipboard bytes before the thread-start gap', () => {
  beforeEach(() => {
    _resetPendingUploadsForTesting();
    _resetComposeDraftsForTesting();
    vi.mocked(uploadThreadBlob).mockReset();
    vi.mocked(awaitThreadStarted).mockReset();
  });

  afterEach(() => {
    _resetPendingUploadsForTesting();
    _resetComposeDraftsForTesting();
    // Restore the module-mock default (immediate resolve) for other suites.
    vi.mocked(awaitThreadStarted).mockImplementation(async () => {});
  });

  it('reads the source bytes before thread creation completes and uploads an in-memory copy', async () => {
    // Hold thread creation open so we can observe that the byte read already
    // happened while the pasteboard-backed File was still valid — the window
    // during which the old code had NOT yet touched the bytes.
    let startThread!: () => void;
    vi.mocked(awaitThreadStarted).mockImplementation(
      () => new Promise<void>((resolve) => { startThread = () => resolve(); }),
    );

    let uploaded: { threadId: string; file: File } | null = null;
    vi.mocked(uploadThreadBlob).mockImplementation(async (threadId: string, file: File) => {
      uploaded = { threadId, file };
      return { hash: 'sha-materialized', mime: file.type, byte_size: file.size };
    });

    const bytes = new Uint8Array([0xff, 0xd8, 0xff, 0x42]);
    let reads = 0;
    const source = {
      name: 'IMG_0001.jpg',
      type: 'image/jpeg',
      arrayBuffer: async () => { reads++; return bytes.buffer.slice(0); },
    } as unknown as File;

    const attachPromise = attachImageToActiveDraft(source);

    // Let the arrayBuffer snapshot microtask settle. Thread creation is still
    // pending (startThread not called), so the upload has NOT fired yet — but
    // the bytes are already captured. This is the crux of the fix: the read
    // does not wait for the async thread-start gap.
    await new Promise((r) => setTimeout(r, 0));
    expect(reads, 'source bytes must be snapshotted before the thread-start gap').toBe(1);
    expect(uploadThreadBlob).not.toHaveBeenCalled();

    // Now let thread creation finish; the upload runs with the snapshot.
    startThread();
    await attachPromise;

    expect(uploadThreadBlob).toHaveBeenCalledTimes(1);
    expect(uploaded).not.toBeNull();
    // The uploaded File is a fresh in-memory copy, never the clipboard-backed
    // source object.
    expect(uploaded!.file).not.toBe(source);
    const uploadedBytes = new Uint8Array(await uploaded!.file.arrayBuffer());
    expect(Array.from(uploadedBytes)).toEqual(Array.from(bytes));
    expect(getDraft('t-1').image_hashes).toEqual(['sha-materialized']);
  });

  it('surfaces a clear error and uploads nothing when the source bytes are unreadable', async () => {
    vi.mocked(awaitThreadStarted).mockImplementation(async () => {});
    const source = {
      name: 'stale.jpg',
      type: 'image/jpeg',
      // A pasteboard resource already released before we could read it.
      arrayBuffer: async () => { throw new TypeError('Failed to fetch'); },
    } as unknown as File;

    await attachImageToActiveDraft(source);

    expect(uploadThreadBlob).not.toHaveBeenCalled();
    expect(getDraft('t-1').image_hashes).toEqual([]);
    expect(pendingUploads.value.get('t-1')).toBeUndefined();
    expect(toasts.value).toHaveLength(1);
    expect(toasts.value[0].message).toContain('could not read the image');
  });
});

/** Regression: a macOS Chrome paste of a screenshot declared `image/png` and
 *  handed over ZERO bytes. The composer gated on the declared type, so it drew
 *  a chip with a broken thumbnail. The real reason arrived seconds later as an
 *  upload toast. The gate now reads the bytes the way the engine does, before
 *  the chip exists. */
describe('attachImageToActiveDraft refuses bytes the server would refuse', () => {
  /** A source whose declared type is a lie the old gate believed. */
  function sourceOf(name: string, type: string, ...content: number[]): File {
    return {
      name,
      type,
      arrayBuffer: async () => new Uint8Array(content).buffer,
    } as unknown as File;
  }

  let objectUrls: number;

  beforeEach(() => {
    _resetPendingUploadsForTesting();
    _resetComposeDraftsForTesting();
    vi.mocked(uploadThreadBlob).mockReset();
    vi.mocked(awaitThreadStarted).mockImplementation(async () => {});
    objectUrls = 0;
    (globalThis as any).URL.createObjectURL = () => { objectUrls++; return 'blob:fake'; };
  });

  afterEach(() => {
    _resetPendingUploadsForTesting();
    _resetComposeDraftsForTesting();
    (globalThis as any).URL.createObjectURL = () => 'blob:fake';
  });

  /** No chip, no preview URL, no upload, and one toast naming this paste. */
  function expectRefused(fragment: string) {
    expect(objectUrls, 'a refused image must not get a preview URL').toBe(0);
    expect(pendingUploads.value.get('t-1'), 'a refused image must not draw a chip').toBeUndefined();
    expect(uploadThreadBlob).not.toHaveBeenCalled();
    expect(getDraft('t-1').image_hashes).toEqual([]);
    expect(toasts.value).toHaveLength(1);
    expect(toasts.value[0].message).toContain(fragment);
  }

  it('refuses the clipboard shape that failed: image/png declared, zero bytes', async () => {
    await attachImageToActiveDraft(sourceOf('image.png', 'image/png'));
    expectRefused('is empty (0 bytes)');
  });

  it('names TIFF instead of reciting the allowlist', async () => {
    await attachImageToActiveDraft(sourceOf('shot.tiff', 'image/tiff', 0x49, 0x49, 0x2a, 0x00, 0x08));
    expectRefused(`that's a TIFF image`);
  });

  it('refuses bytes it cannot place and reports what declared them', async () => {
    await attachImageToActiveDraft(sourceOf('image.png', 'image/png', 0x13, 0x37, 0x42));
    expectRefused('(labelled image/png)');
  });

  it('never recites the allowlist the 415 used to', async () => {
    await attachImageToActiveDraft(sourceOf('image.png', 'image/png'));
    expect(toasts.value[0].message).not.toContain('allowed:');
  });

  it('still attaches an allowlisted image, and uploads the sniffed mime', async () => {
    let uploaded: File | null = null;
    vi.mocked(uploadThreadBlob).mockImplementation(async (_threadId: string, file: File) => {
      uploaded = file;
      return { hash: 'sha-real-png', mime: file.type, byte_size: file.size };
    });

    // Declared `application/octet-stream`, but the bytes are a PNG. The server
    // would store it, so the composer must not refuse it.
    await attachImageToActiveDraft(
      sourceOf('shot', 'application/octet-stream',
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a),
    );

    expect(toasts.value).toEqual([]);
    expect(uploadThreadBlob).toHaveBeenCalledTimes(1);
    expect(uploaded!.type).toBe('image/png');
    expect(getDraft('t-1').image_hashes).toEqual(['sha-real-png']);
  });
});
