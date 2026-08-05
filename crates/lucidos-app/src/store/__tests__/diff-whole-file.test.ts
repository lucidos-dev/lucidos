import { describe, it, expect, beforeEach } from 'vitest';
import { diffWholeFile, diffWholeFileEffective, panelOverlay, repoDiff, encodeRepoPath } from '../store';
import type { RepoDiff, RepoLocator } from '../store';
import type { Loadable } from '../types';

function setPreview(path: string, mode: 'file' | 'diff', changeId?: string): void {
  const locator: RepoLocator = mode === 'diff'
    ? { repoId: 'repo-1', mode, changeId, path }
    : { repoId: 'repo-1', mode, path };
  panelOverlay.value = { type: 'file-preview', path: encodeRepoPath(locator) };
}

function loadedDiff(files: RepoDiff['files']): Loadable<RepoDiff> {
  return { status: 'loaded', data: { files } };
}

beforeEach(() => {
  diffWholeFile.value = null;
  panelOverlay.value = null;
  repoDiff.value = { status: 'not-loaded' };
});

describe('diffWholeFileEffective', () => {
  it('defaults an added file to the whole-file view (diff is all additions)', () => {
    repoDiff.value = loadedDiff([{ path: 'new.rs', status: 'added', hunks: [] }]);
    setPreview('new.rs', 'diff', 'change-1');
    expect(diffWholeFileEffective.value).toBe(true);
  });

  it('defaults a modified file to the hunks', () => {
    repoDiff.value = loadedDiff([{ path: 'edit.rs', status: 'modified', hunks: [] }]);
    setPreview('edit.rs', 'diff', 'change-1');
    expect(diffWholeFileEffective.value).toBe(false);
  });

  it('defaults a deleted file to the hunks (no end state to show)', () => {
    repoDiff.value = loadedDiff([{ path: 'gone.rs', status: 'deleted', hunks: [] }]);
    setPreview('gone.rs', 'diff', 'change-1');
    expect(diffWholeFileEffective.value).toBe(false);
  });

  it('lets an explicit toggle override the added-file default (back to hunks)', () => {
    repoDiff.value = loadedDiff([{ path: 'new.rs', status: 'added', hunks: [] }]);
    setPreview('new.rs', 'diff', 'change-1');
    diffWholeFile.value = false;
    expect(diffWholeFileEffective.value).toBe(false);
  });

  it('lets an explicit toggle override the modified-file default (to whole file)', () => {
    repoDiff.value = loadedDiff([{ path: 'edit.rs', status: 'modified', hunks: [] }]);
    setPreview('edit.rs', 'diff', 'change-1');
    diffWholeFile.value = true;
    expect(diffWholeFileEffective.value).toBe(true);
  });

  it('is false in plain file mode (not a diff preview)', () => {
    repoDiff.value = loadedDiff([{ path: 'edit.rs', status: 'added', hunks: [] }]);
    setPreview('edit.rs', 'file');
    expect(diffWholeFileEffective.value).toBe(false);
  });

  it('is false while the diff is still loading (status unknown)', () => {
    repoDiff.value = { status: 'loading' };
    setPreview('new.rs', 'diff', 'change-1');
    expect(diffWholeFileEffective.value).toBe(false);
  });

  it('is false when nothing is previewed', () => {
    repoDiff.value = loadedDiff([{ path: 'new.rs', status: 'added', hunks: [] }]);
    expect(diffWholeFileEffective.value).toBe(false);
  });
});
