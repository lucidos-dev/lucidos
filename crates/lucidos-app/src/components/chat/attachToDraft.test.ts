import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';

(globalThis as any).URL.createObjectURL = () => 'blob:fake';
(globalThis as any).URL.revokeObjectURL = () => {};

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
import { removeAttachedImage, _resetSessionBlobUrlsForTesting } from './pastedImages';

function makeFakeFile(name: string, type: string): File {
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

    await vi.waitFor(() => expect(uploadThreadBlob).toHaveBeenCalledTimes(1));

    const pendingForThread = pendingUploads.value.get('t-1') ?? [];
    expect(pendingForThread).toHaveLength(1);
    const localId = pendingForThread[0].localId;

    removePendingUpload('t-1', localId);
    expect(pendingUploads.value.get('t-1')).toBeUndefined();
    expect(getDraft('t-1').image_hashes).toEqual([]);

    resolveUpload({ hash: 'sha256-of-photo', mime: 'image/png', byte_size: 1 });
    await attachPromise;

    expect(getDraft('t-1').image_hashes).toEqual([]);
    expect(pendingUploads.value.get('t-1')).toBeUndefined();
  });
});

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
    vi.mocked(awaitThreadStarted).mockImplementation(async () => {});
  });

  it('reads the source bytes before thread creation completes and uploads an in-memory copy', async () => {
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

    await new Promise((r) => setTimeout(r, 0));
    expect(reads, 'source bytes must be snapshotted before the thread-start gap').toBe(1);
    expect(uploadThreadBlob).not.toHaveBeenCalled();

    startThread();
    await attachPromise;

    expect(uploadThreadBlob).toHaveBeenCalledTimes(1);
    expect(uploaded).not.toBeNull();
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

describe('attachImageToActiveDraft refuses bytes the server would refuse', () => {
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

describe('a draft holds each image once', () => {
  const HASH = 'sha256-of-the-screenshot';

  function pngSource(name: string): File {
    return {
      name,
      type: 'image/png',
      arrayBuffer: async () =>
        new Uint8Array([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]).buffer,
    } as unknown as File;
  }

  beforeEach(() => {
    _resetPendingUploadsForTesting();
    _resetComposeDraftsForTesting();
    _resetSessionBlobUrlsForTesting();
    vi.mocked(uploadThreadBlob).mockReset();
    vi.mocked(uploadThreadBlob).mockImplementation(async (_threadId: string, file: File) => ({
      hash: HASH,
      mime: file.type,
      byte_size: 8,
    }));
    vi.mocked(awaitThreadStarted).mockImplementation(async () => {});
  });

  afterEach(() => {
    _resetPendingUploadsForTesting();
    _resetComposeDraftsForTesting();
    _resetSessionBlobUrlsForTesting();
  });

  it('attaches the same bytes once, and says why the second went nowhere', async () => {
    await attachImageToActiveDraft(pngSource('shot.png'));
    expect(getDraft('t-1').image_hashes).toEqual([HASH]);
    expect(toasts.value).toEqual([]);

    await attachImageToActiveDraft(pngSource('shot.png'));

    expect(getDraft('t-1').image_hashes).toEqual([HASH]);
    expect(pendingUploads.value.get('t-1')).toBeUndefined();
    expect(toasts.value).toHaveLength(1);
    expect(toasts.value[0].message).toContain('already attached');
  });

  it('holds one hash when both pastes land together', async () => {
    await Promise.all([
      attachImageToActiveDraft(pngSource('shot.png')),
      attachImageToActiveDraft(pngSource('shot.png')),
    ]);

    expect(getDraft('t-1').image_hashes).toEqual([HASH]);
    expect(pendingUploads.value.get('t-1')).toBeUndefined();
  });

  it('still attaches a second, different image', async () => {
    await attachImageToActiveDraft(pngSource('first.png'));
    vi.mocked(uploadThreadBlob).mockImplementation(async (_threadId: string, file: File) => ({
      hash: 'sha256-of-another-shot',
      mime: file.type,
      byte_size: 8,
    }));

    await attachImageToActiveDraft(pngSource('second.png'));

    expect(getDraft('t-1').image_hashes).toEqual([HASH, 'sha256-of-another-shot']);
    expect(toasts.value).toEqual([]);
  });

  it('takes the image back after the user removed it', async () => {
    await attachImageToActiveDraft(pngSource('shot.png'));
    removeAttachedImage('t-1', 0);
    expect(getDraft('t-1').image_hashes).toEqual([]);

    await attachImageToActiveDraft(pngSource('shot.png'));

    expect(getDraft('t-1').image_hashes).toEqual([HASH]);
    expect(toasts.value).toEqual([]);
  });
});
