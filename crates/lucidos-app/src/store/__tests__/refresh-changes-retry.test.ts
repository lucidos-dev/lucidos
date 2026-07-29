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
    // Simulates the iOS PWA wake case: the first fetch fired by runResumeSync
    // hits the 10s client timeout while the radio is still warming up; a
    // second attempt (on a now-warm connection) succeeds.
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
    // The iOS-PWA-over-Tailscale case: the first fetch fired by runResumeSync on
    // wake hits a stale HTTP/2 connection and rejects with Safari's transport
    // error; a retry on the now-warm connection succeeds. No toast.
    mockFetchChanges
      .mockRejectedValueOnce(new TypeError('Load failed'))
      .mockResolvedValueOnce({ ...emptyState, pending: [{ id: 'c1' } as never] });

    refreshChangesState();
    await vi.waitFor(() => expect(mockFetchChanges).toHaveBeenCalledTimes(2));
    await vi.waitFor(() => expect(loadedPendingLength()).toBe(1));

    expect(toasts.value.find(t => t.message?.startsWith('Failed to fetch changes'))).toBeUndefined();
  });

  it('suppresses the toast (and preserves state) when a transport TypeError fails even the retry', async () => {
    // Both attempts hit the stale connection (Safari "Load failed"). This is
    // transient page-lifecycle / reachability noise on a flaky link, not an
    // outage — the already-loaded list must stay intact (no failed state, no
    // "Failed to fetch changes: Load failed" toast); SSE keeps it live and the
    // next runResumeSync re-syncs. The connection dot owns real outages.
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
    // There is no manual AbortController on this path — an AbortError is the
    // browser cancelling the in-flight fetch when an iOS PWA freezes/backgrounds
    // mid-refresh or the connection resets on a radio handoff. That's transient
    // page-lifecycle noise the next runResumeSync re-syncs, so it must NOT toast
    // ("Failed to fetch changes: request cancelled" was the spurious mobile
    // error) and must NOT clobber the already-loaded list with a failed state.
    changes.value = { status: 'loaded', data: [{ id: 'prev' } as never] };
    appliedChanges.value = { status: 'loaded', data: [] };
    mockFetchChanges.mockRejectedValueOnce(new DOMException('aborted', 'AbortError'));

    refreshChangesState();
    await vi.waitFor(() => expect(mockFetchChanges).toHaveBeenCalledTimes(1));
    // Let any errant catch flush before asserting the negative.
    await Promise.resolve();

    expect(toasts.value.find(t => t.message?.startsWith('Failed to fetch changes'))).toBeUndefined();
    expect(changes.value.status).toBe('loaded');
    expect(loadedPendingLength()).toBe(1);
    expect(appliedChanges.value.status).toBe('loaded');
  });

  it('does not retry on non-timeout errors', async () => {
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
    await vi.waitFor(() => expect(loadedAppliedLength()).toBe(1));

    expect(mockFetchChanges).toHaveBeenCalledTimes(1);
    expect(toasts.value.find(t => t.message?.startsWith('Failed to fetch changes'))).toBeUndefined();
  });
});
