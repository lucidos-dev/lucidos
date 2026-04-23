import { describe, it, expect } from 'vitest';
import { exchangeResponseEvents, exchangeStatus, type Exchange } from '../thread-events';
import type { StoredEvent } from '../thread-events';

function step(seq: number, event: Partial<StoredEvent> & { type: string }): { seq: number; event: StoredEvent } {
  return { seq, event: event as StoredEvent };
}

function exchange(steps: Array<{ seq: number; event: StoredEvent }>): Exchange {
  return {
    userEvent: { type: 'MessageReceived', text: 'pls help' } as StoredEvent,
    userSeq: 0,
    steps,
  };
}

describe('exchangeResponseEvents — UserQuestionAsked rendering', () => {
  it('emits a question ResponseEvent for UserQuestionAsked', () => {
    const ex = exchange([
      step(1, {
        type: 'UserQuestionAsked',
        tool_use_id: 'tu_1',
        cc_session_id: 'sess',
        question: 'Pick one:',
        options: [
          { id: 'opt-0', label: 'Yes' },
          { id: 'opt-1', label: 'No', description: 'Cancel build' },
        ],
      }),
    ]);
    const events = exchangeResponseEvents(ex);
    const q = events.find(e => e.type === 'question');
    expect(q).toBeDefined();
    expect((q as { question: string }).question).toBe('Pick one:');
    expect((q as { tool_use_id: string }).tool_use_id).toBe('tu_1');
    expect((q as { options: { label: string }[] }).options).toHaveLength(2);
    expect((q as { resolved?: unknown }).resolved).toBeUndefined();
  });

  it('marks question resolved when matching UserQuestionAnswered follows', () => {
    const ex = exchange([
      step(1, {
        type: 'UserQuestionAsked',
        tool_use_id: 'tu_1',
        cc_session_id: 'sess',
        question: 'Pick one:',
        options: [{ id: 'opt-0', label: 'Yes' }],
      }),
      step(2, {
        type: 'UserQuestionAnswered',
        tool_use_id: 'tu_1',
        answer: { kind: 'Selected', option_id: 'opt-0' },
      }),
    ]);
    const events = exchangeResponseEvents(ex);
    const q = events.find(e => e.type === 'question') as { resolved?: { kind: string; option_id?: string } };
    expect(q.resolved).toEqual({ kind: 'Selected', option_id: 'opt-0' });
  });

  it('resolves with FreeText when user typed instead of clicked', () => {
    const ex = exchange([
      step(1, {
        type: 'UserQuestionAsked',
        tool_use_id: 'tu_2',
        cc_session_id: 'sess',
        question: 'What now?',
        options: [],
      }),
      step(2, {
        type: 'UserQuestionAnswered',
        tool_use_id: 'tu_2',
        answer: { kind: 'FreeText', text: 'skip the tests' },
      }),
    ]);
    const events = exchangeResponseEvents(ex);
    const q = events.find(e => e.type === 'question') as { resolved?: { kind: string; text?: string } };
    expect(q.resolved).toEqual({ kind: 'FreeText', text: 'skip the tests' });
  });

  it('exchangeStatus reads as done while waiting for an answer (no spinner)', () => {
    const ex = exchange([
      step(1, { type: 'SessionStarted', session_id: 'sess', branch: '' }),
      step(2, { type: 'CodingAgentTextStreamed', text: 'thinking…' }),
      step(3, {
        type: 'UserQuestionAsked',
        tool_use_id: 'tu_3',
        cc_session_id: 'sess',
        question: 'Pick:',
        options: [{ id: 'opt-0', label: 'A' }],
      }),
    ]);
    // CC-mode thread; isLast=true; no streaming buffer.
    expect(exchangeStatus(ex, '', true, false, true)).toBe('done');
  });

  it('exchangeStatus returns to cc-working once CC resumes after answer', () => {
    const ex = exchange([
      step(1, { type: 'SessionStarted', session_id: 'sess', branch: '' }),
      step(2, {
        type: 'UserQuestionAsked',
        tool_use_id: 'tu_4',
        cc_session_id: 'sess',
        question: 'Pick:',
        options: [],
      }),
      step(3, {
        type: 'UserQuestionAnswered',
        tool_use_id: 'tu_4',
        answer: { kind: 'Selected', option_id: 'opt-0' },
      }),
      step(4, { type: 'CodingAgentTextStreamed', text: 'continuing…' }),
    ]);
    expect(exchangeStatus(ex, '', true, false, true)).toBe('cc-working');
  });

  it('ignores unrelated UserQuestionAnswered for a different tool_use_id', () => {
    const ex = exchange([
      step(1, {
        type: 'UserQuestionAsked',
        tool_use_id: 'tu_a',
        cc_session_id: 'sess',
        question: 'A?',
        options: [],
      }),
      step(2, {
        type: 'UserQuestionAnswered',
        tool_use_id: 'tu_other',
        answer: { kind: 'Canceled' },
      }),
    ]);
    const events = exchangeResponseEvents(ex);
    const q = events.find(e => e.type === 'question') as { resolved?: unknown };
    expect(q.resolved).toBeUndefined();
  });
});
