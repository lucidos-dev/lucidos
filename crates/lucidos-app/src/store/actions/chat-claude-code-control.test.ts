/**
 * `sendCodingAgentControl` reports whether the engine recorded a pick. A
 * refusal, such as a pick made while a turn runs, toasts the engine's reason.
 */
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { signal } from '@preact/signals-core';
import type { sendControlRequest as SendControlRequest } from '../../api/client';

const showToast = vi.fn();
const sendControlRequest = vi.fn<typeof SendControlRequest>();

class ApiError extends Error {
  constructor(public httpCode: number, public reason: string) { super(reason); }
}

vi.mock('../store', () => ({
  showToast,
  discardingCCThreadIds: signal(new Set<string>()),
  applyingNowThreadIds: signal(new Map<string, string>()),
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
vi.mock('./threads', () => ({ focusThread: vi.fn() }));

const { sendCodingAgentControl } = await import('./chat-claude-code');

const effort = { subtype: 'set_reasoning_effort', effort: 'low' };

describe('sendCodingAgentControl', () => {
  beforeEach(() => vi.clearAllMocks());

  it('reports a recorded pick', async () => {
    sendControlRequest.mockResolvedValueOnce({ ok: true });
    expect(await sendCodingAgentControl('t1', effort)).toBe(true);
    expect(showToast).not.toHaveBeenCalled();
  });

  it('toasts the engine reason when a running turn refuses the pick', async () => {
    const reason = 'The model and effort stay fixed while a turn runs. Pick again when it ends.';
    sendControlRequest.mockRejectedValueOnce(new ApiError(409, reason));
    expect(await sendCodingAgentControl('t1', effort)).toBe(false);
    expect(showToast).toHaveBeenCalledWith(expect.stringContaining(reason), 'error');
  });
});
