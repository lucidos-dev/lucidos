import { describe, it, expect, beforeEach, vi } from 'vitest';
import { repoFiles, repoChanges, repoSource, repoPending, repoViewMode } from '../store';

const mockListRepoFiles = vi.fn();
const mockGetRepoChanges = vi.fn();
const mockGetChangeDiff = vi.fn();
const mockGetChangeById = vi.fn();

vi.mock(import('../../api/client'), async (importOriginal) => {
  const actual = await importOriginal();
  return {
    ...actual,
    listRepoFiles: (...args: Parameters<typeof actual.listRepoFiles>) => mockListRepoFiles(...args),
    getRepoChanges: (...args: Parameters<typeof actual.getRepoChanges>) => mockGetRepoChanges(...args),
    getChangeDiff: (...args: Parameters<typeof actual.getChangeDiff>) => mockGetChangeDiff(...args),
    getChangeById: (...args: Parameters<typeof actual.getChangeById>) => mockGetChangeById(...args),
  };
});

const { refreshRepoView, switchRepoSource } = await import('./repositories');

beforeEach(() => {
  vi.clearAllMocks();
  repoSource.value = 'repo-1';
  repoPending.value = null;
  repoFiles.value = { status: 'not-loaded' };
  repoChanges.value = { status: 'not-loaded' };
  repoViewMode.value = 'all';
});

describe('refreshRepoView reloads file tree after changes apply', () => {
  it('reloads both repoFiles and repoChanges so a merged ChangeApplied is reflected in the tree', async () => {
    repoFiles.value = { status: 'loaded', data: ['data/apps/old.json', 'README.md'] };

    mockListRepoFiles.mockResolvedValueOnce(['README.md']);
    mockGetRepoChanges.mockResolvedValueOnce({ pending: [], applied: [], has_more: false });

    await refreshRepoView('repo-1');

    expect(mockListRepoFiles).toHaveBeenCalledTimes(1);
    expect(mockGetRepoChanges).toHaveBeenCalledTimes(1);
    expect(repoFiles.value).toEqual({ status: 'loaded', data: ['README.md'] });
  });

  it('defaults to "all" view when switching to a repo with pending changes', async () => {
    // The Lucidos repo always has pending changes from CC sessions; switching
    // to it should still land on All Files, not auto-jump to the Changes tab.
    mockListRepoFiles.mockResolvedValueOnce(['foo.rs']);
    mockGetRepoChanges.mockResolvedValueOnce({ pending: [], applied: [], has_more: false });

    await switchRepoSource('repo-1');

    expect(repoViewMode.value).toBe('all');
  });

  it('lists files from HEAD on default load — not from a pending CC branch', async () => {
    // Old CC branches may still track files (e.g. data/) that main has since
    // removed. The default file tree must reflect the repo's real HEAD, not
    // whatever pending branch happens to be open.
    mockListRepoFiles.mockResolvedValueOnce(['README.md']);
    mockGetRepoChanges.mockResolvedValueOnce({ pending: [], applied: [], has_more: false });

    await switchRepoSource('repo-1');

    expect(mockListRepoFiles).toHaveBeenCalledWith('repo-1', undefined);
  });

  it('uses pending change branch as ref when one is selected, HEAD when not', async () => {
    mockGetRepoChanges.mockResolvedValue({ pending: [], applied: [], has_more: false });

    mockListRepoFiles.mockResolvedValueOnce([]);
    await refreshRepoView('repo-1');
    expect(mockListRepoFiles).toHaveBeenLastCalledWith('repo-1', undefined);

    repoPending.value = {
      branch_name: 'claude-code/some-branch',
      files: [],
      description: '',
      thread_id: null,
    };
    mockListRepoFiles.mockResolvedValueOnce([]);
    await refreshRepoView('repo-1');
    expect(mockListRepoFiles).toHaveBeenLastCalledWith('repo-1', 'claude-code/some-branch');
  });
});
