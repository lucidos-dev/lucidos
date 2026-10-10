import { describe, it, expect, beforeEach, vi, afterEach } from 'vitest';
import { applyingNowThreadIds, archivingThreadIds, changes, toasts } from '../store';

vi.mock('../../api/client', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../../api/client')>();
  return {
    ...actual,
    applyNow: vi.fn(),
    applyChange: vi.fn(),
    fetchChanges: vi.fn(),
  };
});

vi.mock('../../components/chat/scrollState', () => ({
  followSentMessage: vi.fn(),
  stopFollowingBottom: vi.fn(),
}));

import { endClaudeCodeAndApply } from '../actions/chat-claude-code';
import { applyNow, applyChange, fetchChanges, ApiError, type Change, type ChangesState } from '../../api/client';

const mockedApplyNow = vi.mocked(applyNow);
const mockedApplyChange = vi.mocked(applyChange);
const mockedFetchChanges = vi.mocked(fetchChanges);

beforeEach(() => {
  applyingNowThreadIds.value = new Map();
  archivingThreadIds.value = new Set();
  changes.value = { status: 'not-loaded' };
  toasts.value = [];
  vi.clearAllMocks();
  vi.useFakeTimers();
});

afterEach(() => {
  vi.useRealTimers();
});

describe('endClaudeCodeAndApply 409 safety timeout', () => {
  it('clears applyingNowThreadIds after safety timeout on 409', async () => {
    // Simulate: page was reloaded (applyingNowThreadIds is empty),
    // but the backend still holds the change claim for its apply.
    mockedApplyNow.mockRejectedValueOnce(new ApiError(409, 'Already applying'));
    await endClaudeCodeAndApply('thread-1');

    // Still set immediately after 409 (optimistic state was set before the API call)
    expect(applyingNowThreadIds.value.has('thread-1')).toBe(true);

    // After safety timeout (60s), should be cleared
    vi.advanceTimersByTime(60_000);
    expect(applyingNowThreadIds.value.has('thread-1')).toBe(false);
  });

  it('does not clear if SSE resolution event arrives before timeout', async () => {
    mockedApplyNow.mockRejectedValueOnce(new ApiError(409, 'Already applying'));
    await endClaudeCodeAndApply('thread-1');

    // Simulate SSE event (ChangeApplied) clearing the state before timeout
    const next = new Map(applyingNowThreadIds.value);
    next.delete('thread-1');
    applyingNowThreadIds.value = next;

    // Advance past timeout — should not re-add the thread
    vi.advanceTimersByTime(60_000);
    expect(applyingNowThreadIds.value.has('thread-1')).toBe(false);
  });

  it('shows the engine\'s own words when an apply holds the session, and keeps it applying', async () => {
    const message = 'An apply is already in progress for this thread. It finishes on its own';
    mockedApplyNow.mockRejectedValueOnce(
      new ApiError(409, message, { error: message, reason: 'apply_in_progress' }),
    );
    await endClaudeCodeAndApply('thread-1');

    const toast = toasts.value.find((t) => t.key === 'applying-thread-1');
    expect(toast?.message).toContain(message);
    // Its progress is the activity group's to show, so the notice does not spin.
    expect(toast?.spinning).toBeFalsy();
    expect(applyingNowThreadIds.value.has('thread-1')).toBe(true);
  });

  it('never calls a Discard in progress an apply, and drops the applying state', async () => {
    const message = 'A discard is already in progress for this thread. Try again once it has finished';
    mockedApplyNow.mockRejectedValueOnce(
      new ApiError(409, message, { error: message, reason: 'discard_in_progress' }),
    );
    await endClaudeCodeAndApply('thread-1');

    const toast = toasts.value.find((t) => t.key === 'applying-thread-1');
    expect(toast?.message).toContain(message);
    expect(toast?.message).not.toMatch(/applying/i);
    expect(toast?.spinning).toBeFalsy();
    expect(applyingNowThreadIds.value.has('thread-1')).toBe(false);
  });

  it.each([
    ['question_open', 'This thread is waiting for your answer to its question. Answer it first, then apply'],
    ['question_unknown', 'Could not check whether this thread is waiting for your answer. Try again'],
  ])('says Not applied for a %s refusal', async (reason, message) => {
    mockedApplyNow.mockRejectedValueOnce(
      new ApiError(409, message, { error: message, reason }),
    );
    await endClaudeCodeAndApply('thread-1');

    const toast = toasts.value.find((t) => t.key === 'applying-thread-1');
    expect(toast?.message).toContain(message);
    expect(toast?.message).toMatch(/not applied/i);
    expect(toast?.spinning).toBeFalsy();
    expect(applyingNowThreadIds.value.has('thread-1')).toBe(false);
  });

  it('silently returns when already tracked (no API call)', async () => {
    // Pre-set the thread as applying
    applyingNowThreadIds.value = new Map([['thread-1', 'requesting']]);

    await endClaudeCodeAndApply('thread-1');

    // Should not have called the API — early return
    expect(mockedApplyNow).not.toHaveBeenCalled();
  });

  it('refuses to apply while dismiss is in progress — states are mutually exclusive', async () => {
    // Scenario: user clicked Archive (dismiss in progress), apply must not start.
    archivingThreadIds.value = new Set(['thread-1']);

    await endClaudeCodeAndApply('thread-1');

    // Should not have called the API or set applying state
    expect(mockedApplyNow).not.toHaveBeenCalled();
    expect(applyingNowThreadIds.value.has('thread-1')).toBe(false);
  });
});

describe('endClaudeCodeAndApply with no live session', () => {
  const pendingChange = { id: 'change-1', thread_id: 'thread-1', status: 'pending', file_count: 1 } as Change;
  const served = (pending: Change[]): ChangesState => ({
    pending,
    applied: [],
    total_pending: pending.length,
    restart_required: false,
    restart_groups: [],
    client_update_available: false,
    has_more_applied: false,
    apply_all_in_progress: false,
  });

  it('re-reads the changes list rather than trusting a cache that lags the proposal', async () => {
    // The cache predates the proposal. Trusting it said "No pending changes".
    changes.value = { status: 'loaded', data: [] };
    mockedApplyNow.mockRejectedValueOnce(new ApiError(404, 'no session'));
    mockedFetchChanges.mockResolvedValueOnce(served([pendingChange]));
    mockedApplyChange.mockResolvedValueOnce({ status: 'applied' } as Awaited<ReturnType<typeof applyChange>>);

    await endClaudeCodeAndApply('thread-1');

    expect(mockedApplyChange).toHaveBeenCalledWith('change-1');
    expect(toasts.value.some((t) => /no pending changes/i.test(t.message))).toBe(false);
  });

  it('never calls the change absent when the list could not be read', async () => {
    changes.value = { status: 'loaded', data: [] };
    mockedApplyNow.mockRejectedValueOnce(new ApiError(404, 'no session'));
    // Transport failures on both attempts leave the stale cache in place.
    mockedFetchChanges
      .mockRejectedValueOnce(new TypeError('Load failed'))
      .mockRejectedValueOnce(new TypeError('Load failed'));

    await endClaudeCodeAndApply('thread-1');

    expect(mockedApplyChange).not.toHaveBeenCalled();
    const toast = toasts.value.find((t) => t.key === 'applying-thread-1');
    expect(toast?.message).toMatch(/not applied/i);
    expect(toast?.message).not.toMatch(/no pending changes/i);
    expect(applyingNowThreadIds.value.has('thread-1')).toBe(false);
  });
});
