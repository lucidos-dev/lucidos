import { describe, it, expect, beforeEach, vi, afterEach } from 'vitest';
import { applyingNowThreadIds, archivingThreadIds, toasts } from '../store';

vi.mock('../../api/client', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../../api/client')>();
  return {
    ...actual,
    applyNow: vi.fn(),
  };
});

vi.mock('../../components/chat/scrollState', () => ({
  followSentMessage: vi.fn(),
  stopFollowingBottom: vi.fn(),
}));

import { endClaudeCodeAndApply } from '../actions/chat-claude-code';
import { applyNow, ApiError } from '../../api/client';

const mockedApplyNow = vi.mocked(applyNow);

beforeEach(() => {
  applyingNowThreadIds.value = new Map();
  archivingThreadIds.value = new Set();
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

  it('shows the engine\'s own words when an apply holds the session, and keeps the spinner', async () => {
    const message = 'An apply is already in progress for this thread. It finishes on its own';
    mockedApplyNow.mockRejectedValueOnce(
      new ApiError(409, message, { error: message, reason: 'apply_in_progress' }),
    );
    await endClaudeCodeAndApply('thread-1');

    const toast = toasts.value.find((t) => t.key === 'applying-thread-1');
    expect(toast?.message).toContain(message);
    expect(toast?.spinning).toBe(true);
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
