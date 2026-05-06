import { describe, it, expect, beforeEach, vi, afterEach } from 'vitest';
import { applyingNowThreadIds, dismissingThreadIds, toasts } from '../store';

vi.mock('../../api/client', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../../api/client')>();
  return {
    ...actual,
    applyNow: vi.fn(),
  };
});

vi.mock('../../components/chat/scrollState', () => ({
  scrollToBottom: vi.fn(),
}));

import { endClaudeCodeAndApply } from '../actions/chat-claude-code';
import { applyNow, ApiError } from '../../api/client';

const mockedApplyNow = vi.mocked(applyNow);

beforeEach(() => {
  applyingNowThreadIds.value = new Map();
  dismissingThreadIds.value = new Set();
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
    // but backend still has apply_now_in_progress = true.
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

  it('silently returns when already tracked (no API call)', async () => {
    // Pre-set the thread as applying
    applyingNowThreadIds.value = new Map([['thread-1', 'requesting']]);

    await endClaudeCodeAndApply('thread-1');

    // Should not have called the API — early return
    expect(mockedApplyNow).not.toHaveBeenCalled();
  });

  it('refuses to apply while dismiss is in progress — states are mutually exclusive', async () => {
    // Scenario: user clicked Done (dismiss in progress), apply must not start.
    dismissingThreadIds.value = new Set(['thread-1']);

    await endClaudeCodeAndApply('thread-1');

    // Should not have called the API or set applying state
    expect(mockedApplyNow).not.toHaveBeenCalled();
    expect(applyingNowThreadIds.value.has('thread-1')).toBe(false);
  });
});
