import { describe, it, expect, vi, beforeEach } from 'vitest';
import { splitDroppedFiles, attachDroppedFilesToDraft } from './attachToDraft';
import { toasts } from '../../store/store';

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
    expect(split.skipped).toEqual([]);
  });

  it('skips non-image files and records their names', () => {
    const files = makeFakeFileList([
      makeFakeFile('doc.pdf', 'application/pdf'),
      makeFakeFile('notes.txt', 'text/plain'),
    ]);
    const split = splitDroppedFiles(files);
    expect(split.images).toEqual([]);
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
    expect(split.skipped).toEqual(['archive.zip']);
  });

  it('treats files with no MIME type as non-image', () => {
    const split = splitDroppedFiles(makeFakeFileList([makeFakeFile('mystery', '')]));
    expect(split.images).toEqual([]);
    expect(split.skipped).toEqual(['mystery']);
  });
});

describe('attachDroppedFilesToDraft', () => {
  it('does not attach non-image files and shows a toast naming the file', async () => {
    const attachImage = vi.fn().mockResolvedValue(undefined);
    await attachDroppedFilesToDraft(
      makeFakeFileList([makeFakeFile('report.pdf', 'application/pdf')]),
      attachImage,
    );
    expect(attachImage).not.toHaveBeenCalled();
    expect(toasts.value).toHaveLength(1);
    expect(toasts.value[0].message).toContain('report.pdf');
    expect(toasts.value[0].type).toBe('warning');
  });

  it('shows a single aggregated toast when multiple non-image files are dropped', async () => {
    const attachImage = vi.fn().mockResolvedValue(undefined);
    await attachDroppedFilesToDraft(
      makeFakeFileList([
        makeFakeFile('a.pdf', 'application/pdf'),
        makeFakeFile('b.zip', 'application/zip'),
      ]),
      attachImage,
    );
    expect(attachImage).not.toHaveBeenCalled();
    expect(toasts.value).toHaveLength(1);
    expect(toasts.value[0].message).toContain('2 non-image files');
  });

  it('attaches image files via the supplied attacher', async () => {
    const attachImage = vi.fn().mockResolvedValue(undefined);
    const a = makeFakeFile('a.png', 'image/png');
    const b = makeFakeFile('b.jpg', 'image/jpeg');
    await attachDroppedFilesToDraft(makeFakeFileList([a, b]), attachImage);
    expect(attachImage).toHaveBeenCalledTimes(2);
    expect(attachImage).toHaveBeenNthCalledWith(1, a);
    expect(attachImage).toHaveBeenNthCalledWith(2, b);
    expect(toasts.value).toEqual([]);
  });

  it('attaches images and shows toast for skipped files in a mixed drop', async () => {
    const attachImage = vi.fn().mockResolvedValue(undefined);
    const img = makeFakeFile('cat.png', 'image/png');
    await attachDroppedFilesToDraft(
      makeFakeFileList([img, makeFakeFile('notes.pdf', 'application/pdf')]),
      attachImage,
    );
    expect(attachImage).toHaveBeenCalledOnce();
    expect(attachImage).toHaveBeenCalledWith(img);
    expect(toasts.value).toHaveLength(1);
    expect(toasts.value[0].message).toContain('notes.pdf');
  });
});
