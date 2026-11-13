import { describe, it, expect, beforeEach, vi } from 'vitest';
import { restartRequired, restartGroups, toasts } from '../store';
import { RESTART_LS_KEY, RESTART_GROUPS_LS_KEY } from '../actions/chat-changes';

const RESTART_FAILURE_TOAST_KEY = 'restart-required';

const mockFetchChanges = vi.fn();
vi.mock('../../api/client', async () => {
  const actual = await vi.importActual<typeof import('../../api/client')>('../../api/client');
  return {
    ...actual,
    fetchChanges: (...args: unknown[]) => mockFetchChanges(...args),
    applyChange: vi.fn(),
    discardChange: vi.fn(),
    applyAllChanges: vi.fn(),
    discardAllChanges: vi.fn(),
    revertChange: vi.fn(),
    restartEngine: vi.fn(),
  };
});

const { refreshChangesState } = await import('../actions/chat-changes');

beforeEach(() => {
  restartRequired.value = false;
  restartGroups.value = [];
  toasts.value = [];
  localStorage.removeItem(RESTART_LS_KEY);
  localStorage.removeItem(RESTART_GROUPS_LS_KEY);
  mockFetchChanges.mockReset();
});

describe('refreshChangesState backfills restart_groups from API', () => {
  it('populates restartGroups from API restart_groups when local groups are empty', async () => {
    // Page-reload scenario: localStorage was cleared, but the engine still
    // tracks applied-but-not-restarted changes server-side.
    mockFetchChanges.mockResolvedValueOnce({
      pending: [],
      applied: [],
      total_pending: 0,
      restart_required: true,
      restart_groups: [
        { thread_id: 't1', thread_title: 'Fix toast detail', commits: ['feat: a', 'fix: b'] },
        { thread_id: 't2', thread_title: 'Update scheduler', commits: ['refactor: x'] },
      ],
      client_update_available: false,
      has_more_applied: false,
    });

    refreshChangesState();
    await vi.waitFor(() => expect(mockFetchChanges).toHaveBeenCalled());
    await vi.waitFor(() => expect(restartGroups.value.length).toBe(2));

    expect(restartGroups.value).toEqual([
      { threadId: 't1', threadTitle: 'Fix toast detail', commits: ['feat: a', 'fix: b'] },
      { threadId: 't2', threadTitle: 'Update scheduler', commits: ['refactor: x'] },
    ]);

    // No toast is surfaced — the engine "New version available" toast is owned by
    // the poll (engine-update.ts) once the rebuild is `ready`. Backfill's job is to
    // populate the groups signal so the restart confirm dialog has data to show.
    expect(toasts.value.find(t => t.key === RESTART_FAILURE_TOAST_KEY)).toBeFalsy();
  });

  it('clears restartGroups when API returns empty restart_groups', async () => {
    // Engine restarted (or change reverted) — server is no longer tracking
    // any pending restart. The frontend must drop the stale groups.
    restartGroups.value = [
      { threadId: 't1', threadTitle: 'Old', commits: ['feat: stale'] },
    ];
    mockFetchChanges.mockResolvedValueOnce({
      pending: [],
      applied: [],
      total_pending: 0,
      restart_required: false,
      restart_groups: [],
      client_update_available: false,
      has_more_applied: false,
    });

    refreshChangesState();
    await vi.waitFor(() => expect(mockFetchChanges).toHaveBeenCalled());
    await vi.waitFor(() => expect(restartGroups.value.length).toBe(0));

    expect(restartRequired.value).toBe(false);
  });

  it('handles missing restart_groups field gracefully (falls back to empty)', async () => {
    // Older engines may not include the field. Frontend must treat missing as empty,
    // not crash.
    mockFetchChanges.mockResolvedValueOnce({
      pending: [],
      applied: [],
      total_pending: 0,
      restart_required: false,
      client_update_available: false,
      has_more_applied: false,
    });

    refreshChangesState();
    await vi.waitFor(() => expect(mockFetchChanges).toHaveBeenCalled());

    expect(restartGroups.value).toEqual([]);
  });
});
