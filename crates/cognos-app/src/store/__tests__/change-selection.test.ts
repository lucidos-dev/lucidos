import { describe, it, expect, beforeEach, vi } from 'vitest';
import {
  repoSelectedChangeId, repoChanges, repoChangesLoadingMore,
  repoSource, repoDiff, repoPending, repoViewMode, repositories,
  activeMenuItem, panelOverlay,
} from '../store';
import type { Change, RepoChangesState } from '../../api/client';

// Mock the API client
vi.mock('../../api/client', async () => {
  const actual = await vi.importActual<typeof import('../../api/client')>('../../api/client');
  return {
    ...actual,
    getChangeDiff: vi.fn(),
    getRepoChanges: vi.fn(),
    listRepoFiles: vi.fn(),
    getRepoDiff: vi.fn(),
  };
});

import { getChangeDiff, getRepoChanges } from '../../api/client';
import { selectRepoChange, loadRepoChanges, viewChangeDiff } from '../actions/repositories';

const mockChange: Change = {
  id: 'change-1',
  request_id: 'req-1',
  thread_id: 'thread-1',
  thread_title: 'Test Thread',
  branch_name: 'claude-code/test-branch',
  repo_root: '/test/repo',
  description: 'Fix the widget',
  file_count: 3,
  files: ['a.rs', 'b.rs', 'c.rs'],
  requires_restart: false,
  hardened: true,
  status: 'pending',
  created_at: '2026-04-07T10:00:00Z',
  resolved_at: null,
  pre_merge_sha: null,
  post_merge_sha: null,
  commits: [],
};

const mockAppliedChange: Change = {
  ...mockChange,
  id: 'change-2',
  status: 'applied',
  resolved_at: '2026-04-07T11:00:00Z',
  pre_merge_sha: 'abc123',
  post_merge_sha: 'def456',
};

beforeEach(() => {
  repoSelectedChangeId.value = null;
  repoChanges.value = { status: 'not-loaded' };
  repoChangesLoadingMore.value = false;
  repoSource.value = null;
  repoDiff.value = { status: 'not-loaded' };
  repoPending.value = null;
  repoViewMode.value = 'all';
  repositories.value = { status: 'not-loaded' };
  activeMenuItem.value = 'files';
  panelOverlay.value = null;
  vi.clearAllMocks();
});

describe('selectRepoChange', () => {
  it('sets selectedChangeId and loads diff for pending change', async () => {
    repoSource.value = 'repo-1';
    (getChangeDiff as ReturnType<typeof vi.fn>).mockResolvedValue({ files: [{ path: 'a.rs', status: 'modified', hunks: [] }] });

    await selectRepoChange(mockChange);

    expect(repoSelectedChangeId.value).toBe('change-1');
    expect(repoViewMode.value).toBe('changes');
    expect(getChangeDiff).toHaveBeenCalledWith('change-1');
    expect(repoDiff.value).toEqual({
      status: 'loaded',
      data: { files: [{ path: 'a.rs', status: 'modified', hunks: [] }] },
    });
    expect(repoPending.value).toEqual({
      branch_name: 'claude-code/test-branch',
      files: ['a.rs', 'b.rs', 'c.rs'],
      description: 'Fix the widget',
      thread_id: 'thread-1',
    });
  });

  it('sets selectedChangeId and clears repoPending for applied change', async () => {
    repoSource.value = 'repo-1';
    (getChangeDiff as ReturnType<typeof vi.fn>).mockResolvedValue({ files: [] });

    await selectRepoChange(mockAppliedChange);

    expect(repoSelectedChangeId.value).toBe('change-2');
    expect(repoPending.value).toBeNull();
  });

  it('clears selection when passed null', async () => {
    repoSource.value = 'repo-1';
    repoSelectedChangeId.value = 'change-1';
    repoViewMode.value = 'changes';

    await selectRepoChange(null);

    expect(repoSelectedChangeId.value).toBeNull();
    expect(repoDiff.value).toEqual({ status: 'not-loaded' });
    expect(repoPending.value).toBeNull();
    expect(repoViewMode.value).toBe('all');
  });

  it('sets diff to failed state when API errors', async () => {
    repoSource.value = 'repo-1';
    (getChangeDiff as ReturnType<typeof vi.fn>).mockRejectedValue(new Error('not found'));

    await selectRepoChange(mockChange);

    expect(repoDiff.value.status).toBe('failed');
  });
});

describe('loadRepoChanges', () => {
  it('loads and stores repo changes', async () => {
    const data: RepoChangesState = {
      pending: [mockChange],
      applied: [mockAppliedChange],
      has_more: false,
    };
    (getRepoChanges as ReturnType<typeof vi.fn>).mockResolvedValue(data);

    await loadRepoChanges('repo-1');

    expect(repoChanges.value).toEqual({ status: 'loaded', data });
    expect(getRepoChanges).toHaveBeenCalledWith('repo-1', 20);
  });

  it('sets failed state on error', async () => {
    (getRepoChanges as ReturnType<typeof vi.fn>).mockRejectedValue(new Error('DB error'));

    await loadRepoChanges('repo-1');

    expect(repoChanges.value.status).toBe('failed');
  });
});

describe('viewChangeDiff', () => {
  it('switches to files tab and selects change by repo_root match', async () => {
    repositories.value = {
      status: 'loaded',
      data: [{ id: 'repo-1', name: 'Test', path: '/test/repo' }],
    };
    (getChangeDiff as ReturnType<typeof vi.fn>).mockResolvedValue({ files: [] });
    (getRepoChanges as ReturnType<typeof vi.fn>).mockResolvedValue({ pending: [], applied: [], has_more: false });

    await viewChangeDiff(mockChange);

    expect(activeMenuItem.value).toBe('files');
    expect(repoSource.value).toBe('repo-1');
    expect(repoSelectedChangeId.value).toBe('change-1');
  });

  it('clears file-preview overlay so diff overview is visible', async () => {
    repositories.value = {
      status: 'loaded',
      data: [{ id: 'repo-1', name: 'Test', path: '/test/repo' }],
    };
    (getChangeDiff as ReturnType<typeof vi.fn>).mockResolvedValue({ files: [] });
    (getRepoChanges as ReturnType<typeof vi.fn>).mockResolvedValue({ pending: [], applied: [], has_more: false });

    // Simulate a file-preview overlay left open from a previous drill-down
    panelOverlay.value = { type: 'file-preview', path: 'repo:repo-1:diff:src/main.rs' };

    await viewChangeDiff(mockChange);

    expect(panelOverlay.value).toBeNull();
  });

  it('does nothing if repo not found', async () => {
    repositories.value = {
      status: 'loaded',
      data: [{ id: 'repo-2', name: 'Other', path: '/other/repo' }],
    };

    await viewChangeDiff(mockChange);

    expect(repoSelectedChangeId.value).toBeNull();
  });
});
