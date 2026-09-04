import { describe, it, expect, beforeEach, vi } from 'vitest';
import { toasts, applyingChangeIds, applyAllInProgress, threadMap } from '../store';

// Mock the API module before importing the action
vi.mock('../../api/client', () => ({
  applyChange: vi.fn(),
  applyAllChanges: vi.fn(),
  cancelApplyAllChanges: vi.fn(),
}));

// focusThread imports React/Preact-coupled modules (scrollState, navigation) that
// aren't usable in this unit test — stub it so the toast onClick is testable in isolation.
vi.mock('../actions/threads', () => ({
  focusThread: vi.fn(),
}));

import { applySingleChange, applyAllChanges, cancelApplyAllBatch } from '../actions/chat-changes';
import { applyChange as apiApply, applyAllChanges as apiApplyAll, cancelApplyAllChanges as apiCancelApplyAll } from '../../api/client';

const mockedApply = vi.mocked(apiApply);
const mockedApplyAll = vi.mocked(apiApplyAll);
const mockedCancelApplyAll = vi.mocked(apiCancelApplyAll);

beforeEach(() => {
  toasts.value = [];
  applyingChangeIds.value = new Set();
  applyAllInProgress.value = false;
  threadMap.value = new Map();
  vi.clearAllMocks();
});

describe('applySingleChange feedback', () => {
  it('does NOT show an HTTP-response toast on hardening — SSE handler covers it (but tracks applying)', async () => {
    // Apply Now hardening is surfaced by the MissingHardeningDetected SSE
    // handler (see missing-hardening-toast.test.ts), not by this HTTP path —
    // mirroring merge conflict. Toasting here too would double-fire whenever
    // the user is on the hardening thread.
    mockedApply.mockResolvedValue({
      status: 'hardening',
      change_id: 'change-1',
      thread_id: 'thread-123',
      message: 'Hardening started',
      restart_required: false,
      commits_applied: 0,
      files_changed: 0,
      review_thread_id: 'thread-123',
    });

    await applySingleChange('change-1');

    // No HTTP-response toast — the SSE event owns the user-facing cue.
    expect(toasts.value.find(t => t.message.toLowerCase().includes('harden'))).toBeUndefined();

    // Still tracks the change as applying so ChangesPanel shows persistent state
    // (the SSE event carries no change_id, so this stays on the HTTP path).
    expect(applyingChangeIds.value.has('change-1')).toBe(true);
  });

  it('does not show review toast on direct apply (status applied)', async () => {
    mockedApply.mockResolvedValue({
      status: 'applied',
      change_id: 'change-2',
      thread_id: 'thread-2',
      message: 'Change applied.',
      restart_required: false,
      commits_applied: 1,
      files_changed: 1,
      applied_commit: 'a'.repeat(40),
      previous_commit: 'b'.repeat(40),
    });

    await applySingleChange('change-2');

    // No review toast should appear
    const toast = toasts.value.find(t => t.message.toLowerCase().includes('review'));
    expect(toast).toBeFalsy();

    // Should not track as applying
    expect(applyingChangeIds.value.has('change-2')).toBe(false);
  });

  it('does NOT show an HTTP-response toast on conflict — SSE handler covers it', async () => {
    // Apply Now conflicts are surfaced by the MergeConflictDetected SSE
    // handler (see merge-conflict-toast.test.ts), not by this HTTP path.
    // Toasting here too would double-fire whenever the user isn't on the
    // conflict thread.
    mockedApply.mockResolvedValue({
      status: 'conflict',
      change_id: 'change-4',
      thread_id: 'thread-456',
      message: 'Merge conflicts in 2 file(s)',
      restart_required: false,
      commits_applied: 0,
      files_changed: 2,
      conflict_thread_id: 'thread-456',
    });

    await applySingleChange('change-4');

    expect(toasts.value.find(t => t.message.toLowerCase().includes('merge conflict'))).toBeUndefined();
    expect(applyingChangeIds.value.has('change-4')).toBe(false);
  });

  it('shows error toast on failure', async () => {
    mockedApply.mockRejectedValue(new Error('Merge conflict'));

    await applySingleChange('change-3');

    const toast = toasts.value.find(t => t.type === 'error');
    expect(toast).toBeTruthy();
    expect(toast!.message).toContain('Merge conflict');
  });

  it('does not show bare "timeout" toast when client timeout fires', async () => {
    // applyChange's 10-min timeout fires the controller with a TimeoutError
    // reason; errorDetail must render that as "request timed out", not bare
    // "timeout" (the unhelpful pre-refactor label).
    mockedApply.mockRejectedValue(new DOMException('timeout', 'TimeoutError'));

    await applySingleChange('change-timeout');

    const toast = toasts.value.find(t => t.type === 'error');
    expect(toast).toBeTruthy();
    expect(toast!.message).not.toBe('timeout');
    expect(toast!.message).toContain('timed out');
  });

  it('applyAllChanges sets in-progress optimistically on hardening (toast comes from SSE)', async () => {
    mockedApplyAll.mockResolvedValue({
      message: 'Started Apply All — hardening the first change.',
      status: 'hardening',
      batch_id: 'batch-1',
      review_thread_id: 'thread-h',
    });

    await applyAllChanges();

    // Stays "in progress" — only ApplyAllBatchCompleted (SSE) clears it, so the
    // bulk button keeps showing "Applying..." through the multi-minute harden.
    expect(applyAllInProgress.value).toBe(true);
    // No HTTP-response toast — the MissingHardeningDetected SSE handler fires it,
    // uniform with merge conflict and single Apply (see missing-hardening-toast.test.ts).
    expect(toasts.value.find(t => t.message.toLowerCase().includes('harden'))).toBeUndefined();
  });

  it('cancelApplyAllBatch swaps the toast to "Canceling..." and calls the cancel API', async () => {
    mockedCancelApplyAll.mockResolvedValue({ canceled_batches: 1, disarmed: 0 });

    await cancelApplyAllBatch();

    expect(mockedCancelApplyAll).toHaveBeenCalledOnce();
    const toast = toasts.value.find((t) => t.key === 'apply-all-batch');
    expect(toast?.message.toLowerCase()).toContain('cancel');
    expect(toast?.spinning).toBe(true);
    // Action is dropped on the optimistic "Canceling..." toast so a second
    // click can't fire a second cancel.
    expect(toast?.action).toBeUndefined();
  });

  it('cancelApplyAllBatch surfaces an error toast when the cancel request fails', async () => {
    mockedCancelApplyAll.mockRejectedValue(new Error('No Apply All batch is running'));

    await cancelApplyAllBatch();

    expect(toasts.value.some((t) => t.type === 'error')).toBe(true);
  });

  it('applyAllChanges clears the in-progress flag when the request errors', async () => {
    mockedApplyAll.mockRejectedValue(new Error('No pending changes'));

    await applyAllChanges();

    expect(applyAllInProgress.value).toBe(false);
    expect(toasts.value.find(t => t.type === 'error')).toBeTruthy();
  });

  it('does NOT show an HTTP-response toast when the batch stops at a conflict — SSE handler covers it', async () => {
    // Apply All conflicts are surfaced by the MergeConflictDetected SSE handler
    // (see merge-conflict-toast.test.ts), uniform with single Apply and Apply
    // All's hardening case. The SSE toast is keyed and transitions in place to
    // "resolved" once the conflict is fixed; the old unkeyed HTTP toast here
    // could never be reached by that resolver, so it dangled forever as a stale
    // "resolving automatically" warning after the batch had already applied.
    threadMap.value = new Map([
      ['thread-X', { meta: { id: 'thread-X', title: 'Big refactor' } } as any],
    ]);
    mockedApplyAll.mockResolvedValue({
      message: 'Started Apply All — first change hit a conflict, recovery is running.',
      restart_required: false,
      conflict_thread_id: 'thread-X',
      applied: 0,
      failed: 0,
    });
    await applyAllChanges();
    expect(toasts.value.find(t => t.message.toLowerCase().includes('merge conflict'))).toBeUndefined();
    // The optimistic bulk-busy flag is untouched by the conflict response — only
    // ApplyAllBatchCompleted (SSE) clears it.
    expect(applyAllInProgress.value).toBe(true);
  });
});
