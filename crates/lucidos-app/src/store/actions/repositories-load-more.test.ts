import { describe, it, expect, beforeEach, vi } from 'vitest';
import { repoChanges, repoChangesLoadingMore, repoSource } from '../store';
import type { Change } from '../../api/client';

const mockGetRepoChanges = vi.fn();

vi.mock(import('../../api/client'), async (importOriginal) => {
  const actual = await importOriginal();
  return {
    ...actual,
    getRepoChanges: (...args: Parameters<typeof actual.getRepoChanges>) => mockGetRepoChanges(...args),
  };
});

const { loadMoreRepoChanges } = await import('./repositories');

function applied(id: string, resolvedAt: string): Change {
  return { id, status: 'applied', resolved_at: resolvedAt } as unknown as Change;
}

function appliedIds(): string[] {
  const list = repoChanges.value;
  return list.status === 'loaded' ? list.data.applied.map((c) => c.id) : [];
}

/** A page request the test settles by hand, so it can move the list first. */
function heldPage(): (value: unknown) => void {
  let release!: (value: unknown) => void;
  mockGetRepoChanges.mockImplementationOnce(() => new Promise((resolve) => { release = resolve; }));
  return (value) => release(value);
}

beforeEach(() => {
  mockGetRepoChanges.mockReset();
  repoChangesLoadingMore.value = false;
  repoSource.value = 'repo-a';
  repoChanges.value = {
    status: 'loaded',
    data: { pending: [], applied: [applied('a1', '2026-01-01T00:00:00Z')], has_more: true },
  };
});

describe('loadMoreRepoChanges against a list that moves under it', () => {
  it('does not write repo A\'s page into the panel after a switch to repo B', async () => {
    const releasePage = heldPage();
    const loading = loadMoreRepoChanges();

    repoSource.value = 'repo-b';
    const repoB = {
      status: 'loaded' as const,
      data: { pending: [], applied: [applied('b1', '2026-01-05T00:00:00Z')], has_more: false },
    };
    repoChanges.value = repoB;
    releasePage({ pending: [], applied: [applied('a0', '2025-12-31T00:00:00Z')], has_more: false });
    await loading;

    expect(repoChanges.value).toBe(repoB);
    expect(repoChangesLoadingMore.value).toBe(false);
  });

  it('keeps a change a refresh brought in while the page was in flight', async () => {
    const releasePage = heldPage();
    mockGetRepoChanges.mockResolvedValueOnce({
      pending: [],
      applied: [applied('a0', '2025-12-31T00:00:00Z')],
      has_more: false,
    });
    const loading = loadMoreRepoChanges();

    // `refreshRepoView` lands after an apply, leading with the new change.
    repoChanges.value = {
      status: 'loaded',
      data: {
        pending: [],
        applied: [applied('a2', '2026-01-02T00:00:00Z'), applied('a1', '2026-01-01T00:00:00Z')],
        has_more: true,
      },
    };
    releasePage({ pending: [], applied: [applied('a0', '2025-12-31T00:00:00Z')], has_more: false });
    await loading;

    expect(appliedIds()).toEqual(['a2', 'a1', 'a0']);
    expect(mockGetRepoChanges).toHaveBeenCalledTimes(2);
  });

  it('appends the page once when nothing moved', async () => {
    mockGetRepoChanges.mockResolvedValueOnce({
      pending: [],
      applied: [applied('a0', '2025-12-31T00:00:00Z')],
      has_more: false,
    });

    await loadMoreRepoChanges();

    expect(appliedIds()).toEqual(['a1', 'a0']);
    expect(mockGetRepoChanges).toHaveBeenCalledWith('repo-a', 20, new Date('2026-01-01T00:00:00Z').getTime() / 1000);
  });
});
