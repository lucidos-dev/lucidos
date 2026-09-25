import { describe, it, expect, beforeEach, vi } from 'vitest';
import { effect } from '@preact/signals';
import {
  repoSelectedChangeId, repoChanges, repoChangesLoadingMore,
  repoSource, repoDiff, repoPending, repoViewMode, repositories, repoFiles,
  activeMenuItem, panelOverlay, SELECTED_CHANGE_KEY,
  threadMap, toasts,
} from '../store';
import '../effects';
import type { Change, RepoChangesState } from '../../api/client';

// Mock the API client
vi.mock('../../api/client', async () => {
  const actual = await vi.importActual<typeof import('../../api/client')>('../../api/client');
  return {
    ...actual,
    getChangeDiff: vi.fn(),
    getChangeById: vi.fn(),
    getRepoChanges: vi.fn(),
    listRepoFiles: vi.fn(),
    getThreadCcDiff: vi.fn(),
  };
});

import { getChangeById, getChangeDiff, getRepoChanges, listRepoFiles, getThreadCcDiff, ApiError } from '../../api/client';
import {
  selectRepoChange, loadRepoChanges, viewChangeDiff, viewThreadCcDiff,
  restoreRepoSelectionFromStorage, switchRepoSource, loadRepoFiles,
} from '../actions/repositories';

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
  incomplete: false,
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
  threadMap.value = new Map();
  toasts.value = [];
  localStorage.removeItem(SELECTED_CHANGE_KEY);
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
    repoSource.value = 'repo-1';

    await loadRepoChanges('repo-1');

    expect(repoChanges.value).toEqual({ status: 'loaded', data });
    expect(getRepoChanges).toHaveBeenCalledWith('repo-1', 20);
  });

  it('sets failed state on error', async () => {
    (getRepoChanges as ReturnType<typeof vi.fn>).mockRejectedValue(new Error('DB error'));
    repoSource.value = 'repo-1';

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

  // App coding-agent changes use the workspace root as repo_root (not a
  // registered Repository), as do changes whose repo was later removed. Both
  // render the diff inline instead of bailing — the All-Files tab needs a
  // registered repo, but the diff itself does not.
  it('renders the diff inline when no registered repo matches', async () => {
    repositories.value = {
      status: 'loaded',
      data: [{ id: 'repo-2', name: 'Other', path: '/other/repo' }],
    };
    (getChangeDiff as ReturnType<typeof vi.fn>).mockResolvedValue({
      files: [
        { path: 'data/apps/widget/index.html', status: 'modified', hunks: [] },
        { path: 'data/apps/widget/app.js', status: 'modified', hunks: [] },
      ],
    });

    await viewChangeDiff(mockChange);

    expect(getChangeDiff).toHaveBeenCalledWith('change-1');
    expect(activeMenuItem.value).toBe('files');
    expect(repoSource.value).toBeNull();
    expect(repoSelectedChangeId.value).toBe('change-1');
    expect(repoViewMode.value).toBe('changes');
    expect(repoDiff.value.status).toBe('loaded');
    // No threadMap entry → no app-id suffix, just the change description.
    expect(repoPending.value?.description).toBe('Fix the widget');
  });

  it('appends the app id to the description for an app coding-agent change', async () => {
    repositories.value = { status: 'loaded', data: [] };
    threadMap.value = new Map([
      ['thread-1', { meta: { codingAgentKind: 'app', codingAgentFolder: '/ws/data/apps/widget/' } } as never],
    ]);
    (getChangeDiff as ReturnType<typeof vi.fn>).mockResolvedValue({
      files: [{ path: 'data/apps/widget/index.html', status: 'modified', hunks: [] }],
    });

    await viewChangeDiff(mockChange);

    expect(repoPending.value?.description).toBe('Fix the widget (widget)');
  });

  it('renders a single-file unregistered change inline (no file-preview overlay)', async () => {
    repositories.value = { status: 'loaded', data: [] };
    (getChangeDiff as ReturnType<typeof vi.fn>).mockResolvedValue({
      files: [{ path: 'data/apps/widget/solo.js', status: 'modified', hunks: [] }],
    });

    await viewChangeDiff(mockChange);

    // A diff never opens a single file directly; unregistered ones render inline.
    expect(panelOverlay.value).toBeNull();
    expect(repoViewMode.value).toBe('changes');
    expect(repoSource.value).toBeNull();
  });

  it('surfaces a failed diff fetch for an unregistered change', async () => {
    repositories.value = { status: 'loaded', data: [] };
    (getChangeDiff as ReturnType<typeof vi.fn>).mockRejectedValue(new Error('git diff failed'));

    await viewChangeDiff(mockChange);

    expect(repoDiff.value.status).toBe('failed');
    expect(toasts.value.find(t => t.type === 'error')).toBeTruthy();
  });

  it('lands on the file list even when the change touches a single file', async () => {
    repositories.value = {
      status: 'loaded',
      data: [{ id: 'repo-1', name: 'Test', path: '/test/repo' }],
    };
    (getChangeDiff as ReturnType<typeof vi.fn>).mockResolvedValue({ files: [{ path: 'solo.rs', status: 'modified', hunks: [] }] });
    (getRepoChanges as ReturnType<typeof vi.fn>).mockResolvedValue({ pending: [], applied: [], has_more: false });

    await viewChangeDiff(mockChange);

    expect(panelOverlay.value).toBeNull();
    expect(repoViewMode.value).toBe('changes');
    expect(repoDiff.value.status).toBe('loaded');
  });

  it('switches to the Files panel already in the diff view, before any fetch lands', () => {
    repositories.value = {
      status: 'loaded',
      data: [{ id: 'repo-1', name: 'Test', path: '/test/repo' }],
    };
    activeMenuItem.value = 'apps';
    (getChangeDiff as ReturnType<typeof vi.fn>).mockReturnValue(new Promise(() => {}));
    (getRepoChanges as ReturnType<typeof vi.fn>).mockReturnValue(new Promise(() => {}));

    void viewChangeDiff(mockChange);

    expect(activeMenuItem.value).toBe('files');
    expect(repoViewMode.value).toBe('changes');
    expect(repoSelectedChangeId.value).toBe('change-1');
    expect(repoDiff.value.status).toBe('loading');
  });

  it('never passes through the All Files view while binding a new repo', async () => {
    repositories.value = {
      status: 'loaded',
      data: [{ id: 'repo-1', name: 'Test', path: '/test/repo' }],
    };
    repoSource.value = 'repo-other';
    (getChangeDiff as ReturnType<typeof vi.fn>).mockResolvedValue({ files: [{ path: 'a.rs', status: 'modified', hunks: [] }] });
    (getRepoChanges as ReturnType<typeof vi.fn>).mockResolvedValue({ pending: [mockChange], applied: [], has_more: false });
    (listRepoFiles as ReturnType<typeof vi.fn>).mockResolvedValue(['a.rs']);

    const modes: string[] = [];
    const stop = effect(() => { modes.push(repoViewMode.value); });
    modes.length = 0; // drop the value the effect read on subscribe
    await viewChangeDiff(mockChange);
    stop();

    expect(modes).toEqual(['changes']);
    expect(repoSource.value).toBe('repo-1');
  });

  it('shows the diff only once a newly bound repo has its change list', async () => {
    repositories.value = {
      status: 'loaded',
      data: [{ id: 'repo-1', name: 'Test', path: '/test/repo' }],
    };
    let resolveChanges!: (v: RepoChangesState) => void;
    (getRepoChanges as ReturnType<typeof vi.fn>).mockReturnValue(new Promise(r => { resolveChanges = r; }));
    (getChangeDiff as ReturnType<typeof vi.fn>).mockResolvedValue({ files: [] });
    (listRepoFiles as ReturnType<typeof vi.fn>).mockResolvedValue([]);

    const done = viewChangeDiff(mockChange);
    await new Promise(r => setTimeout(r, 0));
    expect(repoDiff.value.status).toBe('loading');

    resolveChanges({ pending: [mockChange], applied: [], has_more: false });
    await done;
    expect(repoChanges.value.status).toBe('loaded');
    expect(repoDiff.value.status).toBe('loaded');
  });

  it('drops a diff that lands after the user opened another change', async () => {
    repositories.value = {
      status: 'loaded',
      data: [{ id: 'repo-1', name: 'Test', path: '/test/repo' }],
    };
    repoSource.value = 'repo-1';
    let resolveFirst!: (v: { files: unknown[] }) => void;
    (getChangeDiff as ReturnType<typeof vi.fn>)
      .mockReturnValueOnce(new Promise(r => { resolveFirst = r; }))
      .mockResolvedValueOnce({ files: [{ path: 'second.rs', status: 'modified', hunks: [] }] });
    (listRepoFiles as ReturnType<typeof vi.fn>).mockResolvedValue([]);

    const first = viewChangeDiff(mockChange);
    await new Promise(r => setTimeout(r, 0)); // first is now waiting on its diff
    await viewChangeDiff(mockAppliedChange);
    resolveFirst({ files: [{ path: 'first.rs', status: 'modified', hunks: [] }] });
    await first;

    expect(repoSelectedChangeId.value).toBe('change-2');
    expect(repoDiff.value).toEqual({
      status: 'loaded',
      data: { files: [{ path: 'second.rs', status: 'modified', hunks: [] }] },
    });
  });

  it('lands on the file list (no direct open) for a multi-file change', async () => {
    repositories.value = {
      status: 'loaded',
      data: [{ id: 'repo-1', name: 'Test', path: '/test/repo' }],
    };
    (getChangeDiff as ReturnType<typeof vi.fn>).mockResolvedValue({ files: [
      { path: 'a.rs', status: 'modified', hunks: [] },
      { path: 'b.rs', status: 'added', hunks: [] },
    ] });
    (getRepoChanges as ReturnType<typeof vi.fn>).mockResolvedValue({ pending: [], applied: [], has_more: false });

    await viewChangeDiff(mockChange);

    expect(panelOverlay.value).toBeNull();
    expect(repoViewMode.value).toBe('changes');
  });
});

describe('viewThreadCcDiff', () => {
  // The thread-level Diff button (WaitingBanner / standalone CC diff button)
  // routes here, NOT through viewChangeDiff, so the file-list landing must
  // hold here too.
  it('lands on the file list even for a single-file branch diff', async () => {
    repositories.value = {
      status: 'loaded',
      data: [{ id: 'repo-1', name: 'Test', path: '/test/repo' }],
    };
    (getThreadCcDiff as ReturnType<typeof vi.fn>).mockResolvedValue({
      repo_root: '/test/repo',
      branch_name: 'claude-code/feat',
      base_ref: 'main',
      files: [{ path: 'solo.rs', status: 'added', hunks: [] }],
    });
    (getRepoChanges as ReturnType<typeof vi.fn>).mockResolvedValue({ pending: [], applied: [], has_more: false });
    (listRepoFiles as ReturnType<typeof vi.fn>).mockResolvedValue(['solo.rs']);

    await viewThreadCcDiff('thread-1');

    expect(panelOverlay.value).toBeNull();
    expect(repoViewMode.value).toBe('changes');
    expect(repoDiff.value.status).toBe('loaded');
  });

  it('switches to the Files panel already in the diff view, before the fetch lands', () => {
    repositories.value = { status: 'loaded', data: [] };
    repoSelectedChangeId.value = 'change-1';
    activeMenuItem.value = 'apps';
    (getThreadCcDiff as ReturnType<typeof vi.fn>).mockReturnValue(new Promise(() => {}));

    void viewThreadCcDiff('thread-1');

    expect(activeMenuItem.value).toBe('files');
    expect(repoViewMode.value).toBe('changes');
    expect(repoSelectedChangeId.value).toBeNull();
    expect(repoDiff.value.status).toBe('loading');
  });

  it('drops a thread diff that lands after the user switched repo', async () => {
    repositories.value = {
      status: 'loaded',
      data: [
        { id: 'repo-1', name: 'Test', path: '/test/repo' },
        { id: 'repo-2', name: 'Other', path: '/other/repo' },
      ],
    };
    let resolveDiff!: (v: unknown) => void;
    (getThreadCcDiff as ReturnType<typeof vi.fn>).mockReturnValue(new Promise(r => { resolveDiff = r; }));
    (getRepoChanges as ReturnType<typeof vi.fn>).mockResolvedValue({ pending: [], applied: [], has_more: false });
    (listRepoFiles as ReturnType<typeof vi.fn>).mockResolvedValue([]);

    const pending = viewThreadCcDiff('thread-1');
    await new Promise(r => setTimeout(r, 0)); // now waiting on the thread diff
    await switchRepoSource('repo-2');
    resolveDiff({
      repo_root: '/test/repo',
      branch_name: 'claude-code/feat',
      base_ref: 'main',
      files: [{ path: 'a.rs', status: 'modified', hunks: [] }],
    });
    await pending;

    expect(repoSource.value).toBe('repo-2');
    expect(repoViewMode.value).toBe('all');
    expect(repoPending.value).toBeNull();
    expect(repoDiff.value).toEqual({ status: 'not-loaded' });
  });

  it('drops a change list and file tree for a repo the panel has left', async () => {
    repoSource.value = 'repo-1';
    let resolveChanges!: (v: RepoChangesState) => void;
    let resolveFiles!: (v: string[]) => void;
    (getRepoChanges as ReturnType<typeof vi.fn>).mockReturnValueOnce(new Promise(r => { resolveChanges = r; }));
    (listRepoFiles as ReturnType<typeof vi.fn>).mockReturnValueOnce(new Promise(r => { resolveFiles = r; }));

    const stale = Promise.all([loadRepoChanges('repo-1'), loadRepoFiles('repo-1')]);
    repoSource.value = 'repo-2';
    resolveChanges({ pending: [mockChange], applied: [], has_more: false });
    resolveFiles(['stale.rs']);
    await stale;

    expect(repoChanges.value.status).not.toBe('loaded');
    expect(repoFiles.value.status).not.toBe('loaded');
  });

  it('fails the diff view when the repo is not registered', async () => {
    repositories.value = { status: 'loaded', data: [] };
    (getThreadCcDiff as ReturnType<typeof vi.fn>).mockResolvedValue({
      repo_root: '/unknown/repo',
      branch_name: 'claude-code/feat',
      base_ref: 'main',
      files: [],
    });

    await viewThreadCcDiff('thread-1');

    expect(repoDiff.value.status).toBe('failed');
    expect(toasts.value.find(t => t.type === 'error')).toBeTruthy();
  });

  it('lands on the file list for a multi-file branch diff', async () => {
    repositories.value = {
      status: 'loaded',
      data: [{ id: 'repo-1', name: 'Test', path: '/test/repo' }],
    };
    (getThreadCcDiff as ReturnType<typeof vi.fn>).mockResolvedValue({
      repo_root: '/test/repo',
      branch_name: 'claude-code/feat',
      base_ref: 'main',
      files: [
        { path: 'a.rs', status: 'modified', hunks: [] },
        { path: 'b.rs', status: 'added', hunks: [] },
      ],
    });
    (getRepoChanges as ReturnType<typeof vi.fn>).mockResolvedValue({ pending: [], applied: [], has_more: false });
    (listRepoFiles as ReturnType<typeof vi.fn>).mockResolvedValue(['a.rs', 'b.rs']);

    await viewThreadCcDiff('thread-1');

    expect(panelOverlay.value).toBeNull();
    expect(repoViewMode.value).toBe('changes');
  });
});

describe('selected change persistence', () => {
  it('writes selected change ID to localStorage', async () => {
    repoSource.value = 'repo-1';
    (getChangeDiff as ReturnType<typeof vi.fn>).mockResolvedValue({ files: [] });

    await selectRepoChange(mockChange);

    expect(localStorage.getItem(SELECTED_CHANGE_KEY)).toBe('change-1');
  });

  it('removes localStorage entry when selection is cleared', async () => {
    repoSource.value = 'repo-1';
    repoSelectedChangeId.value = 'change-1';
    localStorage.setItem(SELECTED_CHANGE_KEY, 'change-1');

    await selectRepoChange(null);

    expect(localStorage.getItem(SELECTED_CHANGE_KEY)).toBeNull();
  });
});

describe('restoreRepoSelectionFromStorage', () => {
  it('re-selects change saved in localStorage on reload', async () => {
    repositories.value = {
      status: 'loaded',
      data: [{ id: 'repo-1', name: 'Test', path: '/test/repo' }],
    };
    localStorage.setItem(SELECTED_CHANGE_KEY, 'change-1');
    (getChangeById as ReturnType<typeof vi.fn>).mockResolvedValue(mockChange);
    (getChangeDiff as ReturnType<typeof vi.fn>).mockResolvedValue({ files: [] });
    (getRepoChanges as ReturnType<typeof vi.fn>).mockResolvedValue({ pending: [], applied: [], has_more: false });

    await restoreRepoSelectionFromStorage();

    expect(repoSelectedChangeId.value).toBe('change-1');
    expect(repoViewMode.value).toBe('changes');
    expect(repoSource.value).toBe('repo-1');
  });

  it('does nothing when no saved ID', async () => {
    await restoreRepoSelectionFromStorage();

    expect(repoSelectedChangeId.value).toBeNull();
    expect(getChangeById).not.toHaveBeenCalled();
  });

  it('clears stale ID when change no longer exists (404 from engine)', async () => {
    localStorage.setItem(SELECTED_CHANGE_KEY, 'change-gone');
    (getChangeById as ReturnType<typeof vi.fn>).mockRejectedValue(new ApiError(404, 'Not found'));

    await restoreRepoSelectionFromStorage();

    expect(localStorage.getItem(SELECTED_CHANGE_KEY)).toBeNull();
    expect(repoSelectedChangeId.value).toBeNull();
  });

  it('keeps saved ID on transient failures (network down, 5xx)', async () => {
    // Without this guard, a momentary engine outage would silently lose the
    // user's diff-view selection across the next reload.
    localStorage.setItem(SELECTED_CHANGE_KEY, 'change-still-there');
    (getChangeById as ReturnType<typeof vi.fn>).mockRejectedValue(new Error('Network down'));

    await restoreRepoSelectionFromStorage();

    expect(localStorage.getItem(SELECTED_CHANGE_KEY)).toBe('change-still-there');
  });

  it('skips when a file-preview overlay already encodes the same change', async () => {
    // RepoFilePreview's useEffect handles this case — duplicating the fetch
    // here doubles the round-trip on every reload of a diff file preview.
    localStorage.setItem(SELECTED_CHANGE_KEY, 'change-1');
    panelOverlay.value = {
      type: 'file-preview',
      path: 'repo:repo-1:diff#change-1:src/main.rs',
    };

    await restoreRepoSelectionFromStorage();

    expect(getChangeById).not.toHaveBeenCalled();
  });
});
