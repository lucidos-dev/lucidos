import { describe, it, expect } from 'vitest';
import { threadFlagsChangeDrawing, type FlagSensitivityInputs } from '../thread-flag-sensitivity';
import type { StoredEvent } from '../../thread-events';
import { makeExchange, step } from '../../__tests__/fixtures';

const inputs: FlagSensitivityInputs = {
  streamingBuffer: '',
  isLast: true,
  isQueued: false,
  hasPriorActive: false,
  threadIsCC: true,
  behindOpenQuestion: false,
};

const message: StoredEvent = { type: 'MessageReceived', text: 'hi' };

describe('threadFlagsChangeDrawing: in-place fold', () => {
  it('re-decides a turn whose step is replaced in place under a bumped revision', () => {
    const turn = makeExchange(message, [
      step(1, { type: 'CodingAgentToolCalled', name: 'Bash', args: {} }),
    ]);
    expect(threadFlagsChangeDrawing(turn, inputs)).toBe(true);
    turn.steps[0] = step(1, { type: 'CodingAgentIdled' });
    turn.revision = 1;
    expect(threadFlagsChangeDrawing(turn, inputs)).toBe(false);
  });
});
