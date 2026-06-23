import { describe, it, expect, beforeEach, vi } from 'vitest';
import { applyingNowThreadIds, threadMap, toasts, showToast } from '../store';
import type { ThreadState } from '../thread-events';

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

const baseState = {
  pending: [] as unknown[],
  applied: [] as unknown[],
  total_pending: 0,
  restart_required: false,
  restart_groups: [],
  client_update_available: false,
  has_more_applied: false,
  apply_all_in_progress: false,
};

/** Minimal pending/applied Change row for the reconcile. */
function change(threadId: string, status: string, description = 'feat: work') {
  return {
    id: `change-${threadId}`,
    request_id: 'req',
    thread_id: threadId,
    thread_title: 'T',
    branch_name: 'b',
    repo_root: '/repo',
    description,
    file_count: 1,
    files: ['a.ts'],
    requires_restart: false,
    hardened: true,
    status,
    created_at: '2026-06-23T00:00:00Z',
    resolved_at: status === 'applied' ? '2026-06-23T00:01:00Z' : null,
    pre_merge_sha: null,
    post_merge_sha: null,
    commits: [],
  };
}

/** A thread fixture with a controllable status so the reconcile's mid-turn
 *  guard can be exercised. */
function thread(id: string, status: ThreadState['meta']['status']): ThreadState {
  return {
    meta: { id, status, channel: 'claude_code' },
    pendingUserMessages: [],
  } as unknown as ThreadState;
}

beforeEach(() => {
  applyingNowThreadIds.value = new Map();
  threadMap.value = new Map();
  toasts.value = [];
  mockFetchChanges.mockReset();
});

describe('refreshChangesState reconciles a stranded Apply Now toast on resume', () => {
  it('clears the toast + state when the apply resolved while SSE was missed', async () => {
    // iOS PWA suspend: the optimistic spinner toast was shown on the Apply tap,
    // the apply completed on the backend, but the ChangeApplied SSE was missed.
    applyingNowThreadIds.value = new Map([['t1', 'applying']]);
    threadMap.value = new Map([['t1', thread('t1', 'idle')]]);
    showToast('Applying changes — T', 'info', { key: 'applying-t1', spinning: true });

    // Backend truth: no pending change for the thread; it landed in applied.
    mockFetchChanges.mockResolvedValueOnce({
      ...baseState,
      pending: [],
      applied: [change('t1', 'applied', 'feat: landed work')],
    });

    refreshChangesState();
    await vi.waitFor(() => expect(applyingNowThreadIds.value.has('t1')).toBe(false));
    // The sticky spinner is resolved into a success toast, not left dangling.
    const t = toasts.value.find((x) => x.key === 'applying-t1');
    expect(t?.type).toBe('success');
    expect(t?.spinning).not.toBe(true);
  });

  it('dismisses the toast when the change is gone (discarded) and not applied', async () => {
    applyingNowThreadIds.value = new Map([['t2', 'applying']]);
    threadMap.value = new Map([['t2', thread('t2', 'idle')]]);
    showToast('Applying changes — T', 'info', { key: 'applying-t2', spinning: true });

    mockFetchChanges.mockResolvedValueOnce({ ...baseState, pending: [], applied: [] });

    refreshChangesState();
    await vi.waitFor(() => expect(applyingNowThreadIds.value.has('t2')).toBe(false));
    expect(toasts.value.some((x) => x.key === 'applying-t2')).toBe(false);
  });

  it('keeps the state while the change is still pending (apply in progress)', async () => {
    applyingNowThreadIds.value = new Map([['t3', 'applying']]);
    threadMap.value = new Map([['t3', thread('t3', 'idle')]]);
    showToast('Applying changes — T', 'info', { key: 'applying-t3', spinning: true });

    // Harden/merge still running on the backend → change stays pending.
    mockFetchChanges.mockResolvedValueOnce({
      ...baseState,
      pending: [change('t3', 'pending')],
    });

    refreshChangesState();
    await vi.waitFor(() => expect(mockFetchChanges).toHaveBeenCalled());
    expect(applyingNowThreadIds.value.has('t3')).toBe(true);
    expect(toasts.value.find((x) => x.key === 'applying-t3')?.spinning).toBe(true);
  });

  it('keeps the state while the thread is mid-turn (CC running the apply)', async () => {
    // Pre-proposal / harden window: no pending change yet but CC is running, so
    // the apply has not resolved — do not clear.
    applyingNowThreadIds.value = new Map([['t4', 'requesting']]);
    threadMap.value = new Map([['t4', thread('t4', 'running')]]);
    showToast('Applying changes — T', 'info', { key: 'applying-t4', spinning: true });

    mockFetchChanges.mockResolvedValueOnce({ ...baseState, pending: [], applied: [] });

    refreshChangesState();
    await vi.waitFor(() => expect(mockFetchChanges).toHaveBeenCalled());
    expect(applyingNowThreadIds.value.has('t4')).toBe(true);
  });
});
