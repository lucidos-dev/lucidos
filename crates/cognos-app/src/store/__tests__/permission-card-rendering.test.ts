import { describe, it, expect } from 'vitest';
import { exchangeResponseEvents, exchangeStatus, type Exchange } from '../thread-events';
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

describe('exchangeResponseEvents — CodingAgentPermissionRequest rendering', () => {
  it('emits a permission ResponseEvent for CodingAgentPermissionRequest', () => {
    const ex = exchange([
      permissionRequestStep(1, {
        input: { file_path: '/Users/me/.claude/skills/foo.md' },
        summary: 'Edit /Users/me/.claude/skills/foo.md',
      }),
    ]);
    const events = exchangeResponseEvents(ex);
    const card = events.find(e => e.type === 'permission');
    expect(card).toBeDefined();
    expect((card as { request_id: string }).request_id).toBe('req-1');
    expect((card as { tool_name: string }).tool_name).toBe('Edit');
    expect((card as { summary: string }).summary).toBe('Edit /Users/me/.claude/skills/foo.md');
    expect((card as { resolved?: unknown }).resolved).toBeUndefined();
  });

  it.each([
    { allowed: true, reason: undefined, label: 'allow' },
    { allowed: false, reason: 'User denied', label: 'deny with reason' },
  ])('flips card to resolved=$label when matching CodingAgentPermissionResolved follows', ({ allowed, reason }) => {
    const ex = exchange([
      permissionRequestStep(1, { request_id: 'req-2' }),
      step(2, {
        type: 'CodingAgentPermissionResolved',
        request_id: 'req-2',
        allowed,
        ...(reason ? { reason } : {}),
      }),
    ]);
    const events = exchangeResponseEvents(ex);
    const card = events.find(e => e.type === 'permission') as { resolved?: { allowed: boolean; reason?: string } };
    expect(card.resolved).toEqual({ allowed, reason });
  });

  it('ignores a Resolved event with a non-matching request_id', () => {
    const ex = exchange([
      permissionRequestStep(1, { request_id: 'req-A' }),
      step(2, { type: 'CodingAgentPermissionResolved', request_id: 'req-OTHER', allowed: false }),
    ]);
    const events = exchangeResponseEvents(ex);
    const card = events.find(e => e.type === 'permission') as { resolved?: unknown };
    expect(card.resolved).toBeUndefined();
  });

  it('exchangeStatus reads as done while waiting for permission (no spinner)', () => {
    const ex = exchange([
      step(1, { type: 'SessionStarted', session_id: 'sess', branch: '' }),
      step(2, { type: 'CodingAgentTextStreamed', text: 'editing…' }),
      permissionRequestStep(3, { request_id: 'req-3' }),
    ]);
    expect(exchangeStatus(ex, '', true, false, true)).toBe('done');
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
