import { describe, it, expect, beforeEach, vi } from 'vitest';
import { applyAllBatch, applyAllCanceling, applyAllInProgress, applyPhases, toasts } from '../store';
import { liveActivityRows } from '../actions/activityRows';

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

// Loading effects registers every toast effect, so a test can assert that no
// progress toast follows the rehydrated signal.
await import('../effects');
const { refreshChangesState } = await import('../actions/chat-changes');

const baseState = {
  pending: [],
  applied: [],
  total_pending: 0,
  restart_required: false,
  restart_groups: [],
  client_update_available: false,
  has_more_applied: false,
};

beforeEach(() => {
  applyAllInProgress.value = false;
  applyAllBatch.value = null;
  applyAllCanceling.value = false;
  applyPhases.value = new Map();
  toasts.value = [];
  mockFetchChanges.mockReset();
});

describe('refreshChangesState rehydrates the Apply All batch across reload', () => {
  it('brings back the Apply All row when a batch is live on the engine', async () => {
    // Page-reload scenario: the optimistic signal reset to false on load and the
    // ApplyAllBatchStarted SSE is not replayed, but the engine still has the
    // batch in flight (apply_all_batches row present).
    mockFetchChanges.mockResolvedValueOnce({ ...baseState, apply_all_in_progress: true });

    refreshChangesState();
    await vi.waitFor(() => expect(applyAllInProgress.value).toBe(true));
    expect(liveActivityRows().map((r) => r.key)).toEqual(['apply-all']);
    expect(toasts.value).toEqual([]);
  });

  it('keeps naming the change in flight as "N of M" after a reload', async () => {
    const member = {
      id: 'c-2', request_id: 'r', thread_id: 't-2', thread_title: 'Collapse Menu',
      branch_name: 'b', repo_root: '/tmp/repo', description: 'd', file_count: 1, files: ['a.ts'],
      requires_restart: false, hardened: true, status: 'pending', created_at: '2026-05-04T00:00:00Z',
      resolved_at: null, pre_merge_sha: null, post_merge_sha: null, commits: [], incomplete: false,
      resolving_conflict: true,
    };
    mockFetchChanges.mockResolvedValueOnce({
      ...baseState,
      pending: [member, { ...member, id: 'c-3', thread_id: 't-3', resolving_conflict: false }],
      apply_all_in_progress: true,
      apply_all_batch: { change_ids: ['c-1', 'c-2', 'c-3'], resolved_change_ids: ['c-1'] },
    });

    refreshChangesState();
    await vi.waitFor(() => expect(applyAllBatch.value?.resolvedChangeIds).toEqual(['c-1']));
    const [row] = liveActivityRows();
    expect(row.label).toBe('Applying 3 changes');
    expect(row.detail).toBe('2 of 3');
    expect(row.body).toMatchObject({
      kind: 'apply-all',
      thread: { title: 'Collapse Menu', phase: 'Resolving merge conflict' },
    });
  });

  it('keeps a parked member on its own row after a reload', async () => {
    const member = {
      id: 'c-2', request_id: 'r', thread_id: 't-2', thread_title: 'Collapse Menu',
      branch_name: 'b', repo_root: '/tmp/repo', description: 'd', file_count: 1, files: ['a.ts'],
      requires_restart: false, hardened: true, status: 'pending', created_at: '2026-05-04T00:00:00Z',
      resolved_at: null, pre_merge_sha: null, post_merge_sha: null, commits: [], incomplete: false,
      resolving_conflict: true,
    };
    mockFetchChanges.mockResolvedValueOnce({
      ...baseState,
      pending: [member, { ...member, id: 'c-3', thread_id: 't-3', thread_title: 'Card Gate', resolving_conflict: false }],
      apply_all_in_progress: true,
      apply_all_batch: {
        change_ids: ['c-1', 'c-2', 'c-3'],
        resolved_change_ids: ['c-1'],
        applying_change_ids: ['c-3'],
        resolving_change_ids: ['c-2'],
      },
    });

    refreshChangesState();
    await vi.waitFor(() => expect(applyAllBatch.value?.resolvingChangeIds).toEqual(['c-2']));
    const rows = liveActivityRows();
    expect(rows.map((r) => r.label)).toEqual(['Applying 3 changes', 'Resolving merge conflict: Collapse Menu']);
    expect(rows[0].detail).toBe('3 of 3');
    expect(rows[0].body).toMatchObject({ kind: 'apply-all', thread: { title: 'Card Gate', phase: 'Merging' } });
  });

  it('drops a Canceling the page never saw complete, so the next batch reads normally', async () => {
    applyAllCanceling.value = true;
    mockFetchChanges.mockResolvedValueOnce({ ...baseState, apply_all_in_progress: false });

    refreshChangesState();
    await vi.waitFor(() => expect(applyAllCanceling.value).toBe(false));
  });

  it('drops a phase whose terminal event the page missed', async () => {
    // A conflict set the phase, then the stream dropped the ChangeApplyFailed.
    applyPhases.value = new Map([['t-9', { phase: 'resolving-conflict', eventId: 'e-9', startedAt: null }]]);
    mockFetchChanges.mockResolvedValueOnce({ ...baseState, apply_all_in_progress: false });

    refreshChangesState();
    await vi.waitFor(() => expect(applyPhases.value.size).toBe(0));
    expect(liveActivityRows()).toEqual([]);
    expect(toasts.value).toEqual([]);
  });

  it('clears a stale in-progress flag when no batch is live', async () => {
    applyAllInProgress.value = true;
    mockFetchChanges.mockResolvedValueOnce({ ...baseState, apply_all_in_progress: false });

    refreshChangesState();
    await vi.waitFor(() => expect(applyAllInProgress.value).toBe(false));
    expect(liveActivityRows()).toEqual([]);
  });

  it('treats a missing field (older engine) as not-in-progress', async () => {
    applyAllInProgress.value = true;
    mockFetchChanges.mockResolvedValueOnce({ ...baseState });

    refreshChangesState();
    await vi.waitFor(() => expect(applyAllInProgress.value).toBe(false));
  });
});
