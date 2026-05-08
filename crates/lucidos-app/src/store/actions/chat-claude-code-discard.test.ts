/**
 * Verifies handleDiscardCCChanges fires an optimistic spinning toast keyed by
 * thread, mirroring the Apply Now flow. The SSE-driven ChangeDiscarded handler
 * in thread-sync.ts uses the same key to replace it with the detailed result.
 */
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { signal } from '@preact/signals-core';

const showToast = vi.fn();
const discardingCCThreadIds = signal(new Set<string>());
const changeToastMessage = vi.fn((action: string) => `${action}: t1`);

vi.mock('../store', () => ({
  showToast,
  discardingCCThreadIds,
  applyingNowThreadIds: signal(new Map<string, string>()),
  archivingThreadIds: signal(new Set<string>()),
  changes: signal<unknown[]>([]),
}));

const discardCCChanges = vi.fn(async () => {});
vi.mock('../../api/client', () => ({
  applyNow: vi.fn(),
  applyChange: vi.fn(),
  answerCCQuestion: vi.fn(),
  discardCCChanges,
  sendControlRequest: vi.fn(),
  ApiError: class extends Error {},
}));
vi.mock('../../components/chat/scrollState', () => ({ scrollToBottom: vi.fn() }));
vi.mock('./thread-sync', () => ({ changeToastMessage }));
vi.mock('./threads', () => ({ focusThread: vi.fn() }));

const { handleDiscardCCChanges } = await import('./chat-claude-code');

describe('handleDiscardCCChanges', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    discardingCCThreadIds.value = new Set();
  });

  it('fires a spinning optimistic toast keyed by thread, then defers success to SSE', async () => {
    await handleDiscardCCChanges('t1');
    expect(showToast).toHaveBeenCalledTimes(1);
    const [message, type, opts] = showToast.mock.calls[0];
    expect(message).toBe('Discarding changes: t1');
    expect(type).toBe('info');
    expect(opts).toMatchObject({ key: 'discarding-t1', spinning: true });
    expect(typeof opts.onClick).toBe('function');
  });

  it('replaces the optimistic toast with an error toast (same key) on failure', async () => {
    discardCCChanges.mockRejectedValueOnce(new Error('boom'));
    await handleDiscardCCChanges('t1');
    expect(showToast).toHaveBeenCalledTimes(2);
    const [message, type, opts] = showToast.mock.calls[1];
    expect(message).toContain('Failed to discard changes');
    expect(type).toBe('error');
    expect(opts).toMatchObject({ key: 'discarding-t1' });
  });

  it('clears discardingCCThreadIds after success and after failure', async () => {
    await handleDiscardCCChanges('t1');
    expect(discardingCCThreadIds.value.has('t1')).toBe(false);

    discardCCChanges.mockRejectedValueOnce(new Error('boom'));
    await handleDiscardCCChanges('t1');
    expect(discardingCCThreadIds.value.has('t1')).toBe(false);
  });
});
