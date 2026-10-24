import { describe, it, expect, beforeEach, vi } from 'vitest';
import { appliedChanges, changesHasMore, changesLoadingMore } from '../store';
import type { Change } from '../../api/client';

const mockFetchChanges = vi.fn();
vi.mock('../../api/client', async () => {
  const actual = await vi.importActual<typeof import('../../api/client')>('../../api/client');
  return {
    ...actual,
    fetchChanges: (...args: unknown[]) => mockFetchChanges(...args),
  };
});

const { loadMoreChanges } = await import('../actions/chat-changes');
const { handleGlobalEvent } = await import('../actions/thread-sync');

function applied(id: string, resolvedAt: string): Change {
  return { id, status: 'applied', resolved_at: resolvedAt } as unknown as Change;
}

function appliedIds(): string[] {
  const list = appliedChanges.value;
  return list.status === 'loaded' ? list.data.map((c) => c.id) : [];
}

function page(rows: Change[], hasMore: boolean) {
  return { pending: [], applied: rows, total_pending: 0, has_more_applied: hasMore };
}

beforeEach(() => {
  mockFetchChanges.mockReset();
  changesLoadingMore.value = false;
  changesHasMore.value = true;
  appliedChanges.value = {
    status: 'loaded',
    data: [applied('a2', '2026-01-02T00:00:00Z'), applied('a1', '2026-01-01T00:00:00Z')],
  };
});

describe('loadMoreChanges against a list that moves under it', () => {
  it('keeps a change applied while the page was in flight', async () => {
    let releaseFirst!: (value: unknown) => void;
    mockFetchChanges
      .mockImplementationOnce(() => new Promise((resolve) => { releaseFirst = resolve; }))
      .mockResolvedValueOnce(page([applied('a0', '2025-12-31T00:00:00Z')], false));

    const loading = loadMoreChanges();
    // A change lands while the page is in flight. The frame replaces the list
    // with the newest window, which now leads with `a3`.
    handleGlobalEvent('ChangesUpdated', {
      pending: [],
      applied: [
        applied('a3', '2026-01-03T00:00:00Z'),
        applied('a2', '2026-01-02T00:00:00Z'),
        applied('a1', '2026-01-01T00:00:00Z'),
      ],
    });
    releaseFirst(page([applied('a0', '2025-12-31T00:00:00Z')], false));
    await loading;

    expect(appliedIds()).toEqual(['a3', 'a2', 'a1', 'a0']);
    expect(changesHasMore.value).toBe(false);
    expect(changesLoadingMore.value).toBe(false);
  });

  it('appends the page once when nothing moved', async () => {
    mockFetchChanges.mockResolvedValueOnce(page([applied('a0', '2025-12-31T00:00:00Z')], true));

    await loadMoreChanges();

    expect(mockFetchChanges).toHaveBeenCalledTimes(1);
    expect(appliedIds()).toEqual(['a2', 'a1', 'a0']);
    expect(changesHasMore.value).toBe(true);
  });
});
