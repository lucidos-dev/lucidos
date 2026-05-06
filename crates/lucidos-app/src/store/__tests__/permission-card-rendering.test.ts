import { describe, it, expect } from 'vitest';
import { exchangeStatus, type Exchange } from '../thread-events';
import type { StoredEvent } from '../thread-events';

function step(seq: number, event: Partial<StoredEvent> & { type: string }): { seq: number; event: StoredEvent } {
  return { seq, event: event as StoredEvent };
}

function exchange(steps: Array<{ seq: number; event: StoredEvent }>): Exchange {
  return {
    userEvent: { type: 'MessageReceived', text: 'edit my skill file' } as StoredEvent,
    userSeq: 0,
    steps,
  };
}

function permissionRequestStep(seq: number, overrides: Partial<{
  request_id: string;
  tool_use_id: string;
  tool_name: string;
  input: Record<string, unknown>;
  summary: string;
}> = {}) {
  return step(seq, {
    type: 'CodingAgentPermissionRequest',
    request_id: 'req-1',
    tool_use_id: 'tu_1',
    tool_name: 'Edit',
    input: {},
    summary: 'Edit /tmp/x',
    ...overrides,
  });
}

describe('exchangeStatus around CodingAgentPermissionRequest', () => {
  it('exchangeStatus reads as awaiting-answer while waiting for permission (no spinner, no Done label)', () => {
    const ex = exchange([
      step(1, { type: 'SessionStarted', session_id: 'sess', branch: '' }),
      step(2, { type: 'CodingAgentTextStreamed', text: 'editing…' }),
      permissionRequestStep(3, { request_id: 'req-3' }),
    ]);
    expect(exchangeStatus(ex, '', true, false, true)).toBe('awaiting-answer');
  });

  it('exchangeStatus returns to cc-working once CC resumes after answer', () => {
    const ex = exchange([
      step(1, { type: 'SessionStarted', session_id: 'sess', branch: '' }),
      permissionRequestStep(2, { request_id: 'req-4' }),
      step(3, { type: 'CodingAgentPermissionResolved', request_id: 'req-4', allowed: true }),
      step(4, { type: 'CodingAgentTextStreamed', text: 'continuing…' }),
    ]);
    expect(exchangeStatus(ex, '', true, false, true)).toBe('cc-working');
  });
});
