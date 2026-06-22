import { describe, it, expect, beforeEach, vi } from 'vitest';
import { applyAllInProgress, toasts } from '../store';

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

// Loading effects registers the sticky apply-all-batch toast effect, so we can
// assert the toast follows the rehydrated signal — not just the signal itself.
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
  toasts.value = [];
  mockFetchChanges.mockReset();
});

describe('refreshChangesState rehydrates the Apply All batch toast across reload', () => {
  it('shows the sticky toast when a batch is live on the engine', async () => {
    // Page-reload scenario: the optimistic signal reset to false on load and the
    // ApplyAllBatchStarted SSE is not replayed, but the engine still has the
    // batch in flight (apply_all_batches row present).
    mockFetchChanges.mockResolvedValueOnce({ ...baseState, apply_all_in_progress: true });

    refreshChangesState();
    await vi.waitFor(() => expect(applyAllInProgress.value).toBe(true));
    expect(toasts.value.some((t) => t.key === 'apply-all-batch')).toBe(true);
  });

  it('clears a stale in-progress flag when no batch is live', async () => {
    applyAllInProgress.value = true;
    mockFetchChanges.mockResolvedValueOnce({ ...baseState, apply_all_in_progress: false });

    refreshChangesState();
    await vi.waitFor(() => expect(applyAllInProgress.value).toBe(false));
    expect(toasts.value.some((t) => t.key === 'apply-all-batch')).toBe(false);
  });

  it('treats a missing field (older engine) as not-in-progress', async () => {
    applyAllInProgress.value = true;
    mockFetchChanges.mockResolvedValueOnce({ ...baseState });

    refreshChangesState();
    await vi.waitFor(() => expect(applyAllInProgress.value).toBe(false));
  });
});
