/**
 * `sendCodingAgentControl` passes on when the engine says a change takes
 * effect. A Claude Code effort change made mid-turn lands on the next turn,
 * and the menu toasts that rather than claiming it applied.
 */
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { signal } from '@preact/signals-core';
import type { sendControlRequest as SendControlRequest } from '../../api/client';

const showToast = vi.fn();
// Typed, so every `takes_effect` below is checked against the generated values.
const sendControlRequest = vi.fn<typeof SendControlRequest>();

class ApiError extends Error {
  constructor(public httpCode: number, public reason: string) { super(reason); }
}

vi.mock('../store', () => ({
  showToast,
  discardingCCThreadIds: signal(new Set<string>()),
  applyingNowThreadIds: signal(new Map<string, string>()),
  archivingThreadIds: signal(new Set<string>()),
  changes: signal<unknown[]>([]),
}));
vi.mock('../../api/client', () => ({
  applyNow: vi.fn(),
  applyChange: vi.fn(),
  answerThreadQuestion: vi.fn(),
  discardCCChanges: vi.fn(),
  sendControlRequest,
  ApiError,
}));
vi.mock('../../components/chat/scrollState', () => ({ followSentMessage: vi.fn(), stopFollowingBottom: vi.fn() }));
vi.mock('./threads', () => ({ focusThread: vi.fn() }));

const { sendCodingAgentControl } = await import('./chat-claude-code');

const effort = { subtype: 'set_reasoning_effort', effort: 'low' };

describe('sendCodingAgentControl', () => {
  beforeEach(() => vi.clearAllMocks());

  it('reports a change that applies now as ok', async () => {
    sendControlRequest.mockResolvedValueOnce({ ok: true, takes_effect: 'now' });
    expect(await sendCodingAgentControl('t1', effort)).toBe('ok');
  });

  it('reports a change that waits for the next turn', async () => {
    sendControlRequest.mockResolvedValueOnce({ ok: true, takes_effect: 'next-turn' });
    expect(await sendCodingAgentControl('t1', effort)).toBe('next-turn');
  });

  it('keeps a missing session as a pending pick, with no toast', async () => {
    sendControlRequest.mockRejectedValueOnce(new ApiError(404, 'No session'));
    expect(await sendCodingAgentControl('t1', effort)).toBe('pending');
    expect(showToast).not.toHaveBeenCalled();
  });
});
