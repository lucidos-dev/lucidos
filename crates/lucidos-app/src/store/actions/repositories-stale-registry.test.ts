/**
 * A cache miss on the repository registry is not a verdict.
 *
 * `repositories` is a projection refreshed by `Repository*` SSE, so it goes
 * stale for as long as that frame takes to arrive. `viewThreadCcDiff` used to
 * answer straight off it and toast "is not registered" for a repo that exists,
 * which `.claude/rules/frontend.md` forbids. It now re-reads the registry
 * before concluding, the way `navigateToTrigger` does.
 */
import { describe, it, expect, beforeEach, vi } from 'vitest';
import {
  repositories, repoSource, repoDiff, repoPending, repoViewMode,
  repoFiles, repoChanges, threadMap, toasts,
} from '../store';

const {
  pushNavState, replaceNavState, revealContentPane, getThreadCcDiff, loadRepositoriesMock,
} = vi.hoisted(() => ({
  pushNavState: vi.fn(),
  replaceNavState: vi.fn(),
  revealContentPane: vi.fn(),
  getThreadCcDiff: vi.fn(),
  loadRepositoriesMock: vi.fn(),
}));

vi.mock('./navigation', () => ({ pushNavState, replaceNavState }));
vi.mock('./pane', () => ({ revealContentPane }));
vi.mock('./repositoriesLoader', () => ({ loadRepositories: loadRepositoriesMock }));
vi.mock('../../api/client', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../../api/client')>();
  return {
    ...actual,
    listRepoFiles: vi.fn().mockResolvedValue([]),
    getRepoChanges: vi.fn().mockResolvedValue({ pending: [], applied: [], has_more: false }),
    getChangeDiff: vi.fn(),
    getChangeById: vi.fn(),
    getThreadCcDiff,
  };
});

/** The repo the cached list is missing. The re-read is what finds it, standing
 *  in for the SSE frame that had not arrived yet. */
const REPO = { id: 'repo-9', name: 'example-repo', path: '/repos/example-repo' };

const { viewThreadCcDiff } = await import('./repositories');

describe('viewThreadCcDiff on a stale repository registry', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    toasts.value = [];
    threadMap.value = new Map();
    repoSource.value = null;
    repoDiff.value = { status: 'not-loaded' };
    repoPending.value = null;
    repoViewMode.value = 'all';
    repoFiles.value = { status: 'not-loaded' };
    repoChanges.value = { status: 'not-loaded' };
    loadRepositoriesMock.mockImplementation(async () => {
      repositories.value = { status: 'loaded', data: [REPO] };
    });
    getThreadCcDiff.mockResolvedValue({
      repo_root: REPO.path,
      branch_name: 'coding-agent/some-branch',
      base_ref: 'main',
      files: [
        { path: 'src/a.rs', status: 'modified', hunks: [] },
        { path: 'src/b.rs', status: 'modified', hunks: [] },
      ],
    });
  });

  it('re-reads the registry before reporting a repo as unregistered', async () => {
    // Loaded, and missing the repo: exactly what a registration whose SSE frame
    // has not landed yet looks like.
    repositories.value = { status: 'loaded', data: [] };

    await viewThreadCcDiff('thread-1');

    expect(loadRepositoriesMock, 'a miss must re-read the source').toHaveBeenCalled();
    expect(toasts.value.map((t) => t.message).join(' ')).not.toContain('is not registered');
    expect(repoSource.value).toBe(REPO.id);
    expect(repoDiff.value.status).toBe('loaded');
  });

  it('still reports a repo the re-read also fails to find', async () => {
    repositories.value = { status: 'loaded', data: [] };
    loadRepositoriesMock.mockImplementation(async () => {
      repositories.value = { status: 'loaded', data: [] };
    });

    await viewThreadCcDiff('thread-1');

    expect(toasts.value.some((t) => t.message.includes('is not registered'))).toBe(true);
    expect(repoSource.value).toBeNull();
  });

  it('keeps the cached registry when the re-read fails', async () => {
    const cached = [{ id: 'repo-1', name: 'other', path: '/repos/other' }];
    repositories.value = { status: 'loaded', data: cached };
    loadRepositoriesMock.mockImplementation(async () => {
      repositories.value = { status: 'failed', error: 'network' };
    });

    await viewThreadCcDiff('thread-1');

    expect(repositories.value).toEqual({ status: 'loaded', data: cached });
  });
});
