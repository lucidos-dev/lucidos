import { describe, it, expect, beforeEach, vi } from 'vitest';
import type { Change } from '../../api/client';
import { changes, appliedChanges, lazyChanges } from '../store';

const mockGetChangeById = vi.fn();

vi.mock(import('../../api/client'), async (importOriginal) => {
  const actual = await importOriginal();
  return {
    ...actual,
    getChangeById: (...args: Parameters<typeof actual.getChangeById>) => mockGetChangeById(...args),
  };
});

const { ensureChangeLoaded } = await import('./chat-changes');

function makeChange(id: string, overrides: Partial<Change> = {}): Change {
  return {
    id,
    request_id: 'req-1',
    thread_id: null,
    thread_title: null,
    branch_name: 'claude-code/test',
    repo_root: '/tmp/repo',
    description: 'chore(compose): trim docstrings',
    file_count: 5,
    files: ['a.ts', 'b.ts', 'c.ts', 'd.ts', 'e.ts'],
    requires_restart: false,
    hardened: false,
    status: 'applied',
    created_at: '2026-05-04T00:00:00Z',
    resolved_at: '2026-05-04T00:01:00Z',
    pre_merge_sha: null,
    post_merge_sha: null,
    commits: [],
    incomplete: false,
    ...overrides,
  };
}

beforeEach(() => {
  vi.clearAllMocks();
  changes.value = { status: 'loaded', data: [] };
  appliedChanges.value = { status: 'loaded', data: [] };
  lazyChanges.value = new Map();
});

describe('ensureChangeLoaded', () => {
  it('cache hit: skips fetch when change is already in appliedChanges', async () => {
    appliedChanges.value = { status: 'loaded', data: [makeChange('c-1')] };

    await ensureChangeLoaded('c-1');

    expect(mockGetChangeById).not.toHaveBeenCalled();
    expect(lazyChanges.value.has('c-1')).toBe(false);
  });

  it('cache hit: skips fetch when change is already in pending changes', async () => {
    changes.value = { status: 'loaded', data: [makeChange('c-1', { status: 'pending' })] };

    await ensureChangeLoaded('c-1');

    expect(mockGetChangeById).not.toHaveBeenCalled();
  });

  it('cache hit: skips fetch when change was already lazy-loaded', async () => {
    mockGetChangeById.mockResolvedValueOnce(makeChange('c-1'));
    await ensureChangeLoaded('c-1');
    expect(mockGetChangeById).toHaveBeenCalledTimes(1);

    await ensureChangeLoaded('c-1');
    expect(mockGetChangeById).toHaveBeenCalledTimes(1);
  });

  it('cache miss: fetches once, populates lazyChanges so subsequent renders find desc + fileCount', async () => {
    const change = makeChange('acce637a', {
      description: 'chore(compose): trim docstrings',
      file_count: 5,
    });
    mockGetChangeById.mockResolvedValueOnce(change);

    await ensureChangeLoaded('acce637a');

    expect(mockGetChangeById).toHaveBeenCalledTimes(1);
    expect(mockGetChangeById).toHaveBeenCalledWith('acce637a');
    const loadable = lazyChanges.value.get('acce637a');
    expect(loadable?.status).toBe('loaded');
    if (loadable?.status === 'loaded') {
      expect(loadable.data.description).toBe('chore(compose): trim docstrings');
      expect(loadable.data.file_count).toBe(5);
    }
  });

  it('dedup: two simultaneous lookups for the same id fire only one fetch', async () => {
    let resolveFetch!: (c: Change) => void;
    mockGetChangeById.mockImplementationOnce(
      () => new Promise<Change>(r => { resolveFetch = r; }),
    );

    const p1 = ensureChangeLoaded('c-1');
    const p2 = ensureChangeLoaded('c-1');

    expect(mockGetChangeById).toHaveBeenCalledTimes(1);
    expect(lazyChanges.value.get('c-1')?.status).toBe('loading');

    resolveFetch(makeChange('c-1'));
    await Promise.all([p1, p2]);

    expect(mockGetChangeById).toHaveBeenCalledTimes(1);
    expect(lazyChanges.value.get('c-1')?.status).toBe('loaded');
  });

  it('failed fetch: stores Loadable failed state so the body renders an error and does not refetch', async () => {
    mockGetChangeById.mockRejectedValueOnce(new Error('not found'));
    await ensureChangeLoaded('c-missing');
    expect(mockGetChangeById).toHaveBeenCalledTimes(1);
    expect(lazyChanges.value.get('c-missing')?.status).toBe('failed');

    await ensureChangeLoaded('c-missing');
    expect(mockGetChangeById).toHaveBeenCalledTimes(1);
  });
});
