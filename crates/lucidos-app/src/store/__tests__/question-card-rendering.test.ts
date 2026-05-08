import { describe, it, expect } from 'vitest';
import { exchangeResponseEvents, exchangeStatus, exchangeSteps, type Exchange } from '../thread-events';
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

describe('exchangeStatus + spinner behavior around UserQuestionAsked', () => {
  it('exchangeStatus reads as awaiting-answer while waiting for an answer (no spinner, no Done label)', () => {
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
    expect(exchangeStatus(ex, '', true, false, true)).toBe('awaiting-answer');
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

  it('keeps resume-marker Thinking step spinning when AskUserQuestion ToolResult fires', () => {
    // After UserQuestionAnswered, the engine emits a CodingAgentPromptSent
    // resume marker (empty text → Thinking spinner). Then CC's PreToolUse
    // hook unblocks and CC processes the synthetic tool_result for the
    // AskUserQuestion, which surfaces as CodingAgentToolResult. That
    // CodingAgentToolResult must NOT resolve the resume-marker Thinking
    // spinner — the spinner should keep spinning until CC produces real
    // output (text or a non-AskUserQuestion tool call).
    const ex = exchange([
      step(1, { type: 'SessionStarted', session_id: 'sess', branch: '' }),
      step(2, { type: 'CodingAgentTextStreamed', text: 'thinking…' }),
      step(3, {
        type: 'UserQuestionAsked',
        tool_use_id: 'tu_resume',
        cc_session_id: 'sess',
        question: 'Pick:',
        options: [{ id: 'opt-0', label: 'A' }],
      }),
      step(4, {
        type: 'UserQuestionAnswered',
        tool_use_id: 'tu_resume',
        answer: { kind: 'Selected', option_id: 'opt-0' },
      }),
      step(5, { type: 'CodingAgentPromptSent', text: '' }),
      step(6, { type: 'CodingAgentToolResult', name: '', result: 'opt-0' }),
    ]);

    // The resume-marker Thinking step must still be a spinner (success: null).
    const events = exchangeResponseEvents(ex, 0, true);
    const stepEvents = events.filter(e => e.type === 'step') as Array<{ description: string; success: boolean | null }>;
    const trailingThinking = stepEvents[stepEvents.length - 1];
    expect(trailingThinking.description).toBe('Thinking');
    expect(trailingThinking.success).toBeNull();

    // exchangeSteps (the parallel projection used by other UI surfaces) must agree.
    const steps = exchangeSteps(ex, true, false);
    const lastStep = steps[steps.length - 1];
    expect(lastStep.description).toBe('Thinking');
    expect(lastStep.success).toBeNull();
  });
});
