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

function makeFakeFile(name: string, type: string): File {
  return { name, type } as unknown as File;
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

  it('skips non-image files and records their names', () => {
    const files = makeFakeFileList([
      makeFakeFile('doc.pdf', 'application/pdf'),
      makeFakeFile('notes.txt', 'text/plain'),
    ]);
    const split = splitDroppedFiles(files);
    expect(split.images).toEqual([]);
    expect(split.plugins).toEqual([]);
    expect(split.skipped).toEqual(['doc.pdf', 'notes.txt']);
  });

  it('handles a mix of image and non-image files', () => {
    const img = makeFakeFile('photo.png', 'image/png');
    const files = makeFakeFileList([
      img,
      makeFakeFile('archive.zip', 'application/zip'),
    ]);
    const split = splitDroppedFiles(files);
    expect(split.images).toEqual([img]);
    expect(split.plugins).toEqual([]);
    expect(split.skipped).toEqual(['archive.zip']);
  });

  it('treats files with no MIME type as non-image', () => {
    const split = splitDroppedFiles(makeFakeFileList([makeFakeFile('mystery', '')]));
    expect(split.images).toEqual([]);
    expect(split.plugins).toEqual([]);
    expect(split.skipped).toEqual(['mystery']);
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
    expect(split.skipped).toEqual(['notes.pdf']);
  });
});

describe('attachDroppedFilesToDraft', () => {
  it('does not attach non-image files and shows a toast naming the file', async () => {
    const attachImage = vi.fn().mockResolvedValue(undefined);
    const installPlugin = vi.fn().mockResolvedValue(undefined);
    await attachDroppedFilesToDraft(
      makeFakeFileList([makeFakeFile('report.pdf', 'application/pdf')]),
      attachImage,
      installPlugin,
    );
    expect(attachImage).not.toHaveBeenCalled();
    expect(installPlugin).not.toHaveBeenCalled();
    expect(toasts.value).toHaveLength(1);
    expect(toasts.value[0].message).toContain('report.pdf');
    expect(toasts.value[0].type).toBe('warning');
  });

  it('shows a single aggregated toast when multiple non-image files are dropped', async () => {
    const attachImage = vi.fn().mockResolvedValue(undefined);
    const installPlugin = vi.fn().mockResolvedValue(undefined);
    await attachDroppedFilesToDraft(
      makeFakeFileList([
        makeFakeFile('a.pdf', 'application/pdf'),
        makeFakeFile('b.zip', 'application/zip'),
      ]),
      attachImage,
      installPlugin,
    );
    expect(attachImage).not.toHaveBeenCalled();
    expect(installPlugin).not.toHaveBeenCalled();
    expect(toasts.value).toHaveLength(1);
    expect(toasts.value[0].message).toContain('2 non-image files');
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

  it('attaches images and shows toast for skipped files in a mixed drop', async () => {
    const attachImage = vi.fn().mockResolvedValue(undefined);
    const installPlugin = vi.fn().mockResolvedValue(undefined);
    const img = makeFakeFile('cat.png', 'image/png');
    await attachDroppedFilesToDraft(
      makeFakeFileList([img, makeFakeFile('notes.pdf', 'application/pdf')]),
      attachImage,
      installPlugin,
    );
    expect(attachImage).toHaveBeenCalledOnce();
    expect(attachImage).toHaveBeenCalledWith(img);
    expect(installPlugin).not.toHaveBeenCalled();
    expect(toasts.value).toHaveLength(1);
    expect(toasts.value[0].message).toContain('notes.pdf');
  });

  it('routes .lucidos-plugin drops to the install handler instead of skipping', async () => {
    const attachImage = vi.fn().mockResolvedValue(undefined);
    const installPlugin = vi.fn().mockResolvedValue(undefined);
    const plug = makeFakeFile('thing-0.1.0.lucidos-plugin', 'application/octet-stream');
    await attachDroppedFilesToDraft(
      makeFakeFileList([plug]),
      attachImage,
      installPlugin,
    );
    expect(attachImage).not.toHaveBeenCalled();
    expect(installPlugin).toHaveBeenCalledOnce();
    expect(installPlugin).toHaveBeenCalledWith(plug);
    expect(toasts.value).toEqual([]);
  });

  it('handles a mixed drop of image, plugin, and unsupported file', async () => {
    const attachImage = vi.fn().mockResolvedValue(undefined);
    const installPlugin = vi.fn().mockResolvedValue(undefined);
    const img = makeFakeFile('cat.png', 'image/png');
    const plug = makeFakeFile('p.lucidos-plugin', '');
    const other = makeFakeFile('readme.txt', 'text/plain');
    await attachDroppedFilesToDraft(
      makeFakeFileList([img, plug, other]),
      attachImage,
      installPlugin,
    );
    expect(attachImage).toHaveBeenCalledWith(img);
    expect(installPlugin).toHaveBeenCalledWith(plug);
    expect(toasts.value).toHaveLength(1);
    expect(toasts.value[0].message).toContain('readme.txt');
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

    // Pending preview is rendered; user sees the image.
    const pendingForThread = pendingUploads.value.get('t-1') ?? [];
    expect(pendingForThread).toHaveLength(1);
    const localId = pendingForThread[0].localId;

    // Flush microtasks so the awaited `awaitThreadStarted` resolves and
    // `uploadThreadBlob` is invoked (capturing `resolveUpload`).
    await new Promise((r) => setTimeout(r, 0));
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
