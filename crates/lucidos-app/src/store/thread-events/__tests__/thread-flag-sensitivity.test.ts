import { describe, it, expect } from 'vitest';
import { threadFlagsChangeDrawing, type FlagSensitivityInputs } from '../thread-flag-sensitivity';
import type { Exchange, StoredEvent } from '../../thread-events';
import { makeExchange, step } from '../../__tests__/fixtures';

const inputs: FlagSensitivityInputs = {
  streamingBuffer: '',
  isLast: false,
  isQueued: false,
  hasPriorActive: false,
  threadIsCC: true,
  behindOpenQuestion: false,
};

const message: StoredEvent = { type: 'MessageReceived', text: 'hi' };

describe('threadFlagsChangeDrawing', () => {
  it('is false for a finished coding-agent turn', () => {
    const finished = makeExchange(message, [
      step(1, { type: 'CodingAgentToolCalled', name: 'Bash', args: {} }),
      step(2, { type: 'CodingAgentIdled' }),
    ]);
    expect(threadFlagsChangeDrawing(finished, inputs)).toBe(false);
  });

  it('is true for a live turn with a pending step', () => {
    const live = makeExchange(message, [
      step(1, { type: 'CodingAgentToolCalled', name: 'Bash', args: {} }),
    ]);
    expect(threadFlagsChangeDrawing(live, { ...inputs, isLast: true })).toBe(true);
  });

  const question = {
    type: 'UserQuestionAsked',
    tool_use_id: 'tu-1',
    cc_session_id: 'sess',
    question: 'Wait for the lock?',
    options: [],
  } as Exchange['userEvent'];

  it('is false for a card still waiting on its answer', () => {
    const waiting: Exchange = { userEvent: question, userSeq: 0, steps: [] };
    expect(threadFlagsChangeDrawing(waiting, { ...inputs, isLast: true })).toBe(false);
  });

  it('is true for an answered card whose resume has not ended', () => {
    const answered: Exchange = {
      userEvent: question,
      userSeq: 0,
      steps: [
        step(1, { type: 'UserQuestionAnswered', tool_use_id: 'tu-1', answer: { kind: 'Selected', option_id: 'a' } } as unknown as StoredEvent),
        step(2, { type: 'CodingAgentToolCalled', name: 'Bash', args: {} }),
      ],
    };
    expect(threadFlagsChangeDrawing(answered, { ...inputs, isLast: true })).toBe(true);
  });

  it('is true for a queued turn, whatever its steps', () => {
    const finished = makeExchange(message, [step(1, { type: 'CodingAgentIdled' })]);
    expect(threadFlagsChangeDrawing(finished, { ...inputs, isQueued: true })).toBe(true);
  });

  it('re-decides when the turn gains an event in place', () => {
    const turn = makeExchange(message, [
      step(1, { type: 'CodingAgentToolCalled', name: 'Bash', args: {} }),
    ]);
    const live = { ...inputs, isLast: true };
    expect(threadFlagsChangeDrawing(turn, live)).toBe(true);
    turn.steps.push(step(2, { type: 'CodingAgentIdled' }));
    turn.revision = 1;
    expect(threadFlagsChangeDrawing(turn, live)).toBe(false);
  });
});
