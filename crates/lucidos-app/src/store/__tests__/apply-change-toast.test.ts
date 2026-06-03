import { describe, it, expect, beforeEach, vi } from 'vitest';
import { toasts, applyingChangeIds, threadMap } from '../store';

// Mock the API module before importing the action
vi.mock('../../api/client', () => ({
  applyChange: vi.fn(),
  applyAllChanges: vi.fn(),
}));

// focusThread imports React/Preact-coupled modules (scrollState, navigation) that
// aren't usable in this unit test — stub it so the toast onClick is testable in isolation.
vi.mock('../actions/threads', () => ({
  focusThread: vi.fn(),
}));

import { applySingleChange, applyAllChanges } from '../actions/chat-changes';
import { applyChange as apiApply, applyAllChanges as apiApplyAll } from '../../api/client';

const mockedApply = vi.mocked(apiApply);
const mockedApplyAll = vi.mocked(apiApplyAll);

beforeEach(() => {
  toasts.value = [];
  applyingChangeIds.value = new Set();
  threadMap.value = new Map();
  vi.clearAllMocks();
});

describe('applySingleChange feedback', () => {
  it('shows hardening-in-progress toast when status is hardening', async () => {
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

    // Should show a toast informing the user that hardening is in progress
    const toast = toasts.value.find(t => t.message.toLowerCase().includes('harden'));
    expect(toast).toBeTruthy();
    expect(toast!.type).toBe('info');

    // Should also track the change as applying
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

  it('applyAllChanges shows a toast when the batch stops at a conflict', async () => {
    threadMap.value = new Map([
      ['thread-X', { meta: { id: 'thread-X', title: 'Big refactor' } } as any],
    ]);
    mockedApplyAll.mockResolvedValue({
      message: 'Applied 3 change(s), then hit a conflict.',
      restart_required: false,
      conflict_thread_id: 'thread-X',
      applied: 3,
      failed: 0,
    });
    await applyAllChanges();
    const t = toasts.value.find(t => t.message.toLowerCase().includes('merge conflict'));
    expect(t).toBeTruthy();
    expect(t!.message).toContain('Big refactor');
    expect(t!.message).toContain('3');
  });
});
