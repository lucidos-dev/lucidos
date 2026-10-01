import { describe, it, expect, beforeEach, vi } from 'vitest';
import {
  activeMenuItem, expandedFolders, panelOverlay, repoExpandedFolders, repoFiles,
  repoSource, repoViewMode, revealedFolder,
} from '../store';
import { contentScrollKey } from '../../hooks/useScrollMemory';

const { pushNavState, revealContentPane, listRepoFiles } = vi.hoisted(() => ({
  pushNavState: vi.fn(),
  revealContentPane: vi.fn(),
  listRepoFiles: vi.fn().mockResolvedValue([]),
}));
vi.mock('./navigation', () => ({ pushNavState, replaceNavState: vi.fn() }));
vi.mock('./pane', () => ({ revealContentPane }));

vi.mock('../../api/client', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../../api/client')>();
  return {
    ...actual,
    listRepoFiles,
    getRepoChanges: vi.fn().mockResolvedValue({ changes: [], has_more: false }),
  };
});

const { revealFolderInFiles, switchRepoSource } = await import('./repositories');

describe('revealFolderInFiles', () => {
  beforeEach(() => {
    pushNavState.mockClear();
    revealContentPane.mockClear();
    listRepoFiles.mockClear();
    panelOverlay.value = { type: 'file-preview', path: 'artifacts/examples/note.md' };
    activeMenuItem.value = 'apps';
    repoSource.value = null;
    repoViewMode.value = 'all';
    expandedFolders.value = new Set();
    repoExpandedFolders.value = new Set();
    revealedFolder.value = null;
    localStorage.clear();
  });

  it('lands on the Files panel in one navigation', () => {
    revealFolderInFiles('artifacts/examples/note.md', 'artifacts/examples');

    expect(activeMenuItem.value).toBe('files');
    expect(panelOverlay.value).toBeNull();
    expect(pushNavState).toHaveBeenCalledTimes(1);
    expect(revealContentPane).toHaveBeenCalledTimes(1);
    expect(revealedFolder.value).toBe('artifacts/examples');
  });

  it('opens the folder and every ancestor in the workspace tree', () => {
    revealFolderInFiles('artifacts/a/b/note.md', 'artifacts/a/b');

    expect([...expandedFolders.value].sort()).toEqual(['artifacts', 'artifacts/a', 'artifacts/a/b']);
  });

  it('only adds to the workspace expansion, never closes an unrelated folder', () => {
    expandedFolders.value = new Set(['knowhow']);

    revealFolderInFiles('artifacts/examples/note.md', 'artifacts');

    expect(expandedFolders.value.has('knowhow')).toBe(true);
    expect(expandedFolders.value.has('artifacts')).toBe(true);
  });

  it('switches the panel back to the workspace for a workspace file', () => {
    repoSource.value = 'repo-1';

    revealFolderInFiles('artifacts/examples/note.md', 'artifacts');

    expect(repoSource.value).toBeNull();
  });

  it('binds another repo first, so its expansion survives the bind', () => {
    repoSource.value = 'repo-1';

    revealFolderInFiles('repo:repo-2:file:src/lib/mod.rs', 'src/lib');

    expect(repoSource.value).toBe('repo-2');
    expect(listRepoFiles).toHaveBeenCalledWith('repo-2', undefined);
    expect([...repoExpandedFolders.value].sort()).toEqual(['src', 'src/lib']);
    expect(expandedFolders.value.size).toBe(0);
    expect(revealedFolder.value).toBe('src/lib');
  });

  it('drops a pending reveal when the panel switches to another tree', () => {
    revealFolderInFiles('artifacts/examples/note.md', 'artifacts/examples');

    void switchRepoSource('repo-3');

    expect(revealedFolder.value).toBeNull();
  });

  it('keeps the current repo loaded and moves it to the All Files tree', () => {
    repoSource.value = 'repo-1';
    repoFiles.value = { status: 'loaded', data: ['src/lib/mod.rs'] };
    repoViewMode.value = 'changes';
    repoExpandedFolders.value = new Set(['docs']);

    revealFolderInFiles('repo:repo-1:diff#cid-9:src/lib/mod.rs', 'src');

    expect(listRepoFiles).not.toHaveBeenCalled();
    expect(repoViewMode.value).toBe('all');
    expect([...repoExpandedFolders.value].sort()).toEqual(['docs', 'src']);
  });

  it('forgets the preview it left, so a reload stays on the Files view', () => {
    localStorage.setItem('file-preview-open', 'artifacts/examples/note.md');

    revealFolderInFiles('artifacts/examples/note.md', 'artifacts');

    expect(localStorage.getItem('file-preview-open')).toBeNull();
    expect(localStorage.getItem('lucidos-active-menu-item')).toBe('files');
  });

  it("drops the Files view's remembered scroll, so a restore cannot override the reveal", () => {
    localStorage.setItem(contentScrollKey('files'), '480');

    revealFolderInFiles('artifacts/examples/note.md', 'artifacts');

    expect(localStorage.getItem(contentScrollKey('files'))).toBeNull();
  });
});
