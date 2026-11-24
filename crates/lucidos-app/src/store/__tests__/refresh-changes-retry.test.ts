import { describe, it, expect, beforeEach, vi } from 'vitest';
import { changes, appliedChanges, toasts } from '../store';

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

const emptyState = {
  pending: [],
  applied: [],
  total_pending: 0,
  restart_required: false,
  restart_groups: [],
  client_update_available: false,
  has_more_applied: false,
};

beforeEach(() => {
  changes.value = { status: 'not-loaded' };
  appliedChanges.value = { status: 'not-loaded' };
  toasts.value = [];
  mockFetchChanges.mockReset();
});

function loadedPendingLength(): number {
  return changes.value.status === 'loaded' ? changes.value.data.length : 0;
}
function loadedAppliedLength(): number {
  return appliedChanges.value.status === 'loaded' ? appliedChanges.value.data.length : 0;
}

describe('refreshChangesState transient-wake handling', () => {
  it('retries silently when the first fetch times out and second succeeds', async () => {
    mockFetchChanges
      .mockRejectedValueOnce(new DOMException('timeout', 'TimeoutError'))
      .mockResolvedValueOnce({
        ...emptyState,
        pending: [{ id: 'c1' } as never],
      });

    refreshChangesState();
    await vi.waitFor(() => expect(mockFetchChanges).toHaveBeenCalledTimes(2));
    await vi.waitFor(() => expect(loadedPendingLength()).toBe(1));

    expect(toasts.value.find(t => t.message?.startsWith('Failed to fetch changes'))).toBeUndefined();
  });

  it('shows the toast only when both timeout attempts fail', async () => {
    mockFetchChanges
      .mockRejectedValueOnce(new DOMException('timeout', 'TimeoutError'))
      .mockRejectedValueOnce(new DOMException('timeout', 'TimeoutError'));

    refreshChangesState();
    await vi.waitFor(() => expect(mockFetchChanges).toHaveBeenCalledTimes(2));
    await vi.waitFor(() =>
      expect(toasts.value.find(t => t.message?.startsWith('Failed to fetch changes'))).toBeTruthy(),
    );

    const toast = toasts.value.find(t => t.message?.startsWith('Failed to fetch changes'));
    expect(toast!.message).toBe('Failed to fetch changes: request timed out');
    expect(toast!.type).toBe('error');
  });

  it('retries silently on a transport TypeError ("Load failed") and second succeeds', async () => {
    mockFetchChanges
      .mockRejectedValueOnce(new TypeError('Load failed'))
      .mockResolvedValueOnce({ ...emptyState, pending: [{ id: 'c1' } as never] });

    refreshChangesState();
    await vi.waitFor(() => expect(mockFetchChanges).toHaveBeenCalledTimes(2));
    await vi.waitFor(() => expect(loadedPendingLength()).toBe(1));

    expect(toasts.value.find(t => t.message?.startsWith('Failed to fetch changes'))).toBeUndefined();
  });

  it('suppresses the toast (and preserves state) when a transport TypeError fails even the retry', async () => {
    changes.value = { status: 'loaded', data: [{ id: 'prev' } as never] };
    appliedChanges.value = { status: 'loaded', data: [] };
    mockFetchChanges
      .mockRejectedValueOnce(new TypeError('Load failed'))
      .mockRejectedValueOnce(new TypeError('Load failed'));

    refreshChangesState();
    await vi.waitFor(() => expect(mockFetchChanges).toHaveBeenCalledTimes(2));
    await Promise.resolve();

    expect(toasts.value.find(t => t.message?.startsWith('Failed to fetch changes'))).toBeUndefined();
    expect(changes.value.status).toBe('loaded');
    expect(loadedPendingLength()).toBe(1);
    expect(appliedChanges.value.status).toBe('loaded');
  });

  it('suppresses the toast (and preserves state) on a browser-cancelled AbortError', async () => {
    changes.value = { status: 'loaded', data: [{ id: 'prev' } as never] };
    appliedChanges.value = { status: 'loaded', data: [] };
    mockFetchChanges.mockRejectedValueOnce(new DOMException('aborted', 'AbortError'));

    refreshChangesState();
    await vi.waitFor(() => expect(mockFetchChanges).toHaveBeenCalledTimes(1));
    await Promise.resolve();

    expect(toasts.value.find(t => t.message?.startsWith('Failed to fetch changes'))).toBeUndefined();
    expect(changes.value.status).toBe('loaded');
    expect(loadedPendingLength()).toBe(1);
    expect(appliedChanges.value.status).toBe('loaded');
  });

  it('does not retry on non-timeout errors', async () => {
    mockFetchChanges.mockRejectedValueOnce(new Error('boom'));

    refreshChangesState();
    await vi.waitFor(() =>
      expect(toasts.value.find(t => t.message?.startsWith('Failed to fetch changes'))).toBeTruthy(),
    );

    expect(mockFetchChanges).toHaveBeenCalledTimes(1);
    const toast = toasts.value.find(t => t.message?.startsWith('Failed to fetch changes'));
    expect(toast!.message).toBe('Failed to fetch changes: boom');
  });

  it('does not retry or toast on success', async () => {
    mockFetchChanges.mockResolvedValueOnce({
      ...emptyState,
      applied: [{ id: 'a1' } as never],
    });

    refreshChangesState();
    await vi.waitFor(() => expect(loadedAppliedLength()).toBe(1));

    expect(mockFetchChanges).toHaveBeenCalledTimes(1);
    expect(toasts.value.find(t => t.message?.startsWith('Failed to fetch changes'))).toBeUndefined();
  });
});
