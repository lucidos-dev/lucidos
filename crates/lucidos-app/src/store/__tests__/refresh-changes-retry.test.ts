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
  changes.value = [];
  appliedChanges.value = [];
  toasts.value = [];
  mockFetchChanges.mockReset();
});

describe('refreshChangesState retries once on AbortError', () => {
  it('retries silently when the first fetch aborts and second succeeds', async () => {
    // Simulates the iOS PWA wake case: the first fetch fired by runResumeSync
    // hits the 10s client timeout while the radio is still warming up; a
    // second attempt (on a now-warm connection) succeeds.
    mockFetchChanges
      .mockRejectedValueOnce(new DOMException('Aborted', 'AbortError'))
      .mockResolvedValueOnce({
        ...emptyState,
        pending: [{ id: 'c1' } as never],
      });

    refreshChangesState();
    await vi.waitFor(() => expect(mockFetchChanges).toHaveBeenCalledTimes(2));
    await vi.waitFor(() => expect(changes.value.length).toBe(1));

    expect(toasts.value.find(t => t.message?.startsWith('Failed to fetch changes'))).toBeUndefined();
  });

  it('shows the toast only when both attempts fail', async () => {
    mockFetchChanges
      .mockRejectedValueOnce(new DOMException('Aborted', 'AbortError'))
      .mockRejectedValueOnce(new DOMException('Aborted', 'AbortError'));

    refreshChangesState();
    await vi.waitFor(() => expect(mockFetchChanges).toHaveBeenCalledTimes(2));
    await vi.waitFor(() =>
      expect(toasts.value.find(t => t.message?.startsWith('Failed to fetch changes'))).toBeTruthy(),
    );

    const toast = toasts.value.find(t => t.message?.startsWith('Failed to fetch changes'));
    expect(toast!.message).toBe('Failed to fetch changes: request timed out');
    expect(toast!.type).toBe('error');
  });

  it('does not retry on non-abort errors', async () => {
    // A genuine error (e.g. 500) should surface immediately without a second
    // wasted request — retrying won't help and would just delay the toast.
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
    await vi.waitFor(() => expect(appliedChanges.value.length).toBe(1));

    expect(mockFetchChanges).toHaveBeenCalledTimes(1);
    expect(toasts.value.find(t => t.message?.startsWith('Failed to fetch changes'))).toBeUndefined();
  });
});
