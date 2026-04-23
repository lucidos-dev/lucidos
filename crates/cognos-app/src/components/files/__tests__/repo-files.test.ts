import { describe, it, expect, beforeEach, vi } from 'vitest';
import { buildFolderTree } from '../../../store/actions/artifacts';
import { changeBadgeLabel } from '../changeBadge';

describe('changeBadgeLabel', () => {
  it('returns M for modified', () => {
    expect(changeBadgeLabel('modified')).toBe('M');
  });

  it('returns A for added', () => {
    expect(changeBadgeLabel('added')).toBe('A');
  });

  it('returns D for deleted', () => {
    expect(changeBadgeLabel('deleted')).toBe('D');
  });

  it('returns ? for unknown status', () => {
    expect(changeBadgeLabel('renamed')).toBe('?');
  });
});

describe('buildFolderTree', () => {
  it('returns empty tree for empty input', () => {
    const tree = buildFolderTree([]);
    expect(tree.children).toEqual({});
    expect(tree.files).toEqual([]);
  });

  it('handles root-level files', () => {
    const tree = buildFolderTree(['README.md', 'Cargo.toml']);
    expect(tree.files).toHaveLength(2);
    expect(tree.files[0]).toEqual({ name: 'README.md', path: 'README.md' });
    expect(tree.files[1]).toEqual({ name: 'Cargo.toml', path: 'Cargo.toml' });
    expect(Object.keys(tree.children)).toHaveLength(0);
  });

  it('creates nested folder structure', () => {
    const tree = buildFolderTree(['src/main.rs', 'src/lib.rs']);
    expect(tree.children.src).toBeDefined();
    expect(tree.children.src.name).toBe('src');
    expect(tree.children.src.path).toBe('src');
    expect(tree.children.src.files).toHaveLength(2);
    expect(tree.children.src.files[0]).toEqual({ name: 'main.rs', path: 'src/main.rs' });
  });

  it('creates deeply nested folders with correct paths', () => {
    const tree = buildFolderTree(['a/b/c/file.txt']);
    expect(tree.children.a.path).toBe('a');
    expect(tree.children.a.children.b.path).toBe('a/b');
    expect(tree.children.a.children.b.children.c.path).toBe('a/b/c');
    expect(tree.children.a.children.b.children.c.files[0]).toEqual({
      name: 'file.txt',
      path: 'a/b/c/file.txt',
    });
  });

  it('groups files from same folder correctly', () => {
    const tree = buildFolderTree([
      'src/api/client.ts',
      'src/api/types.ts',
      'src/utils/format.ts',
      'README.md',
    ]);
    expect(tree.files).toHaveLength(1); // README.md at root
    expect(tree.children.src.children.api.files).toHaveLength(2);
    expect(tree.children.src.children.utils.files).toHaveLength(1);
  });

  it('handles mixed depth paths', () => {
    const tree = buildFolderTree([
      'shallow.txt',
      'a/medium.txt',
      'a/b/c/deep.txt',
    ]);
    expect(tree.files).toHaveLength(1);
    expect(tree.children.a.files).toHaveLength(1);
    expect(tree.children.a.children.b.children.c.files).toHaveLength(1);
  });
});

describe('repo store actions', () => {
  // Dynamic imports to avoid signal initialization order issues
  let repoSource: typeof import('../../../store/store').repoSource;
  let repoFiles: typeof import('../../../store/store').repoFiles;
  let repoDiff: typeof import('../../../store/store').repoDiff;
  let repoPending: typeof import('../../../store/store').repoPending;
  let repoViewMode: typeof import('../../../store/store').repoViewMode;
  let repoExpandedFolders: typeof import('../../../store/store').repoExpandedFolders;
  let selectedLines: typeof import('../../../store/store').selectedLines;
  let toggleRepoFolder: typeof import('../../../store/actions/repositories').toggleRepoFolder;
  let expandAllRepoFolders: typeof import('../../../store/actions/repositories').expandAllRepoFolders;
  let collapseAllRepoFolders: typeof import('../../../store/actions/repositories').collapseAllRepoFolders;

  beforeEach(async () => {
    vi.resetModules();
    const store = await import('../../../store/store');
    const actions = await import('../../../store/actions/repositories');
    repoSource = store.repoSource;
    repoFiles = store.repoFiles;
    repoDiff = store.repoDiff;
    repoPending = store.repoPending;
    repoViewMode = store.repoViewMode;
    repoExpandedFolders = store.repoExpandedFolders;
    selectedLines = store.selectedLines;
    toggleRepoFolder = actions.toggleRepoFolder;
    expandAllRepoFolders = actions.expandAllRepoFolders;
    collapseAllRepoFolders = actions.collapseAllRepoFolders;

    // Reset to defaults
    repoSource.value = null;
    repoFiles.value = { status: 'not-loaded' };
    repoDiff.value = { status: 'not-loaded' };
    repoPending.value = null;
    repoViewMode.value = 'all';
    repoExpandedFolders.value = new Set();
    selectedLines.value = null;
  });

  describe('toggleRepoFolder', () => {
    it('expands a collapsed folder', () => {
      toggleRepoFolder('src');
      expect(repoExpandedFolders.value.has('src')).toBe(true);
    });

    it('collapses an expanded folder', () => {
      repoExpandedFolders.value = new Set(['src']);
      toggleRepoFolder('src');
      expect(repoExpandedFolders.value.has('src')).toBe(false);
    });

    it('preserves other expanded folders', () => {
      repoExpandedFolders.value = new Set(['src', 'tests']);
      toggleRepoFolder('src');
      expect(repoExpandedFolders.value.has('src')).toBe(false);
      expect(repoExpandedFolders.value.has('tests')).toBe(true);
    });
  });

  describe('expandAllRepoFolders', () => {
    it('does nothing when files not loaded', () => {
      repoFiles.value = { status: 'loading' };
      expandAllRepoFolders();
      expect(repoExpandedFolders.value.size).toBe(0);
    });

    it('expands all folder paths from loaded files', () => {
      repoFiles.value = {
        status: 'loaded',
        data: ['src/api/client.ts', 'src/utils/format.ts', 'tests/e2e.ts'],
      };
      expandAllRepoFolders();
      expect(repoExpandedFolders.value.has('src')).toBe(true);
      expect(repoExpandedFolders.value.has('src/api')).toBe(true);
      expect(repoExpandedFolders.value.has('src/utils')).toBe(true);
      expect(repoExpandedFolders.value.has('tests')).toBe(true);
    });

    it('does not include root-level files as folders', () => {
      repoFiles.value = { status: 'loaded', data: ['README.md'] };
      expandAllRepoFolders();
      expect(repoExpandedFolders.value.size).toBe(0);
    });
  });

  describe('collapseAllRepoFolders', () => {
    it('clears all expanded folders', () => {
      repoExpandedFolders.value = new Set(['src', 'src/api', 'tests']);
      collapseAllRepoFolders();
      expect(repoExpandedFolders.value.size).toBe(0);
    });
  });
});
