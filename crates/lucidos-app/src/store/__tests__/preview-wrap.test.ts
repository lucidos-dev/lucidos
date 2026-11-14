import { describe, it, expect, beforeEach } from 'vitest';
import {
  panelOverlay,
  repoDiff,
  repoPending,
  repoSelectedChangeId,
  diffWholeFile,
  filePreviewSource,
  filePreviewEditing,
  filePreviewWrap,
  encodeRepoPath,
  type DiffFile,
  type RepoLocator,
} from '../store';
import { previewShowsSource, wrapToggleAvailable } from '../previewWrap';

/** The Wrap toggle acts on the line-numbered source view and on nothing else. A
 *  control offered over a picture or a rendered document would be inert, which
 *  is the same rule `sideBySideDiffAvailable` follows. */
describe('previewShowsSource', () => {
  const ask = (path: string, over: Partial<Parameters<typeof previewShowsSource>[1]> = {}) =>
    previewShowsSource(path, { sourceToggle: false, editing: false, diffShowsWholeFile: true, ...over });

  it('is true for a workspace code file', () => {
    expect(ask('artifacts/main.rs')).toBe(true);
  });

  it('is false for a rendered document, a picture and the editor', () => {
    expect(ask('artifacts/report.md')).toBe(false);
    expect(ask('artifacts/photo.png')).toBe(false);
    expect(ask('artifacts/notes.txt', { editing: true })).toBe(false);
  });

  it('follows the Source toggle onto a rendered document', () => {
    expect(ask('artifacts/report.md', { sourceToggle: true })).toBe(true);
  });

  it('is true for a repo file locator holding source', () => {
    expect(ask('repo:repo-1:file:src/main.rs')).toBe(true);
    expect(ask('repo:repo-1:file#main:src/main.rs')).toBe(true);
    expect(ask('repo:repo-1:file:README.md')).toBe(false);
  });

  // A diff locator renders the hunks in the Files panel and the whole file in
  // the preview modal. So the surface answers this, not the locator.
  it('follows the surface for a diff locator', () => {
    const diff = 'repo:repo-1:diff#change-7:src/main.rs';
    expect(ask(diff, { diffShowsWholeFile: true })).toBe(true);
    expect(ask(diff, { diffShowsWholeFile: false })).toBe(false);
  });
});

describe('wrapToggleAvailable: the content pane header', () => {
  const modified = (path: string): DiffFile => ({ path, status: 'modified', hunks: [] });

  beforeEach(() => {
    panelOverlay.value = null;
    repoDiff.value = { status: 'not-loaded' };
    repoPending.value = null;
    repoSelectedChangeId.value = null;
    diffWholeFile.value = null;
    filePreviewSource.value = false;
    filePreviewEditing.value = false;
  });

  it('is off with no preview showing', () => {
    expect(wrapToggleAvailable.value).toBe(false);
  });

  it('is on over a workspace source file', () => {
    panelOverlay.value = { type: 'file-preview', path: 'artifacts/main.rs' };
    expect(wrapToggleAvailable.value).toBe(true);
  });

  it('is off over a rendered markdown artifact, and on once Source is', () => {
    panelOverlay.value = { type: 'file-preview', path: 'artifacts/report.md' };
    expect(wrapToggleAvailable.value).toBe(false);
    filePreviewSource.value = true;
    expect(wrapToggleAvailable.value).toBe(true);
  });

  it('is off while the inline editor has the file', () => {
    panelOverlay.value = { type: 'file-preview', path: 'artifacts/notes.txt' };
    filePreviewEditing.value = true;
    expect(wrapToggleAvailable.value).toBe(false);
  });

  // The hunks are their own view and wrap on their own (`.diff-line-content`).
  // Only the whole-file body is the line-numbered source this acts on.
  it('is off over the hunks of a diff, and on over its whole file', () => {
    const file = modified('src/main.rs');
    repoDiff.value = { status: 'loaded', data: { files: [file] } };
    const locator: RepoLocator = { repoId: 'repo-1', mode: 'diff', changeId: 'change-7', path: file.path };
    panelOverlay.value = { type: 'file-preview', path: encodeRepoPath(locator) };
    expect(wrapToggleAvailable.value).toBe(false);
    diffWholeFile.value = true;
    expect(wrapToggleAvailable.value).toBe(true);
  });
});

/** The mode is a way of READING a file, not a per-file override, so it survives
 *  the preview moving on and the page reloading. Same class as the side-by-side
 *  diff toggle, and out of the per-file reset for the same reason. */
describe('the wrap mode is remembered', () => {
  it('defaults to wrapping, which is what makes a clipped tail reachable', () => {
    expect(localStorage.getItem('lucidos-file-preview-wrap')).not.toBe('false');
    expect(filePreviewWrap.peek()).toBe(true);
  });

  it('writes every change through to localStorage', async () => {
    await import('../effects');
    filePreviewWrap.value = false;
    expect(localStorage.getItem('lucidos-file-preview-wrap')).toBe('false');
    filePreviewWrap.value = true;
    expect(localStorage.getItem('lucidos-file-preview-wrap')).toBe('true');
  });
});
