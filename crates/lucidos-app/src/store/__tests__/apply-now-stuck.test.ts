import { describe, it, expect, beforeEach, vi } from 'vitest';
import { applyingNowThreadIds, applyNowRequests, threadMap, toasts } from '../store';

vi.mock('../../api/client', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../../api/client')>();
  return {
    ...actual,
    applyNow: vi.fn(),
  };
});


import { endClaudeCodeAndApply } from '../actions/chat-claude-code';
import { applyNow, ApiError } from '../../api/client';
import { _holdArchiveForTesting, _resetRowIntentsForTesting } from '../rowIntents';

const mockedApplyNow = vi.mocked(applyNow);

beforeEach(() => {
  applyNowRequests.value = new Map();
  threadMap.value = new Map();
  _resetRowIntentsForTesting();
  toasts.value = [];
  vi.clearAllMocks();
});

describe('endClaudeCodeAndApply refusals', () => {
  // A running apply shows on the row (`applyInFlight`), so the click's own
  // bridge ends with the refusal and nothing waits on a timer (ADR 0437).
  it('shows the engine\'s own words when an apply holds the session', async () => {
    const message = 'An apply is already in progress for this thread. It finishes on its own';
    mockedApplyNow.mockRejectedValueOnce(
      new ApiError(409, message, { error: message, reason: 'apply_in_progress' }),
    );
    await endClaudeCodeAndApply('thread-1');

    const toast = toasts.value.find((t) => t.key === 'applying-thread-1');
    expect(toast?.message).toContain(message);
    // Its progress is the activity group's to show, so the notice does not spin.
    expect(toast?.spinning).toBeFalsy();
    expect(applyNowRequests.value.has('thread-1')).toBe(false);
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
    applyNowRequests.value = new Map([['thread-1', null]]);

    await endClaudeCodeAndApply('thread-1');

    // Should not have called the API — early return
    expect(mockedApplyNow).not.toHaveBeenCalled();
  });

  it('refuses to apply while dismiss is in progress — states are mutually exclusive', async () => {
    // Scenario: user clicked Archive (dismiss in progress), apply must not start.
    _holdArchiveForTesting(['thread-1']);

    await endClaudeCodeAndApply('thread-1');

    // Should not have called the API or set applying state
    expect(mockedApplyNow).not.toHaveBeenCalled();
    expect(applyingNowThreadIds.value.has('thread-1')).toBe(false);
  });
});
