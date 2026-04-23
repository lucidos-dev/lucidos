import { describe, it, expect, beforeEach, vi } from 'vitest';
import { toasts, applyingChangeIds } from '../store';

// Mock the API module before importing the action
vi.mock('../../api/client', () => ({
  applyChange: vi.fn(),
}));

import { applySingleChange } from '../actions/chat-changes';
import { applyChange as apiApply } from '../../api/client';

const mockedApply = vi.mocked(apiApply);

beforeEach(() => {
  toasts.value = [];
  applyingChangeIds.value = new Set();
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

  it('shows merge conflict toast when status is conflict', async () => {
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

    const toast = toasts.value.find(t => t.message.toLowerCase().includes('merge conflict'));
    expect(toast).toBeTruthy();
    expect(toast!.type).toBe('warning');

    // Conflict does not add to applyingChangeIds — that's handled by MergeConflictDetected SSE
    expect(applyingChangeIds.value.has('change-4')).toBe(false);
  });

  it('shows error toast on failure', async () => {
    mockedApply.mockRejectedValue(new Error('Merge conflict'));

    await applySingleChange('change-3');

    const toast = toasts.value.find(t => t.type === 'error');
    expect(toast).toBeTruthy();
    expect(toast!.message).toContain('Merge conflict');
  });
});
