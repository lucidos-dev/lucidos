import { describe, it, expect } from 'vitest';
import { groupIntoExchanges, sortEventsChronologically, type StoredEvent } from '../thread-events';
import { findQuestionAnswer } from '../thread-events/exchange';

function twoQuestionTurn(): Map<number, StoredEvent> {
  return new Map<number, StoredEvent>([
    [246, { type: 'MessageReceived', text: 'go', _eventId: 'msg-1', created: '2026-08-28T07:19:20.980Z' } as StoredEvent],
    [247, { type: 'SessionStarted', session_id: 's1', created: '2026-08-28T07:19:20.990Z' } as StoredEvent],
    [248, { type: 'UserQuestionAsked', tool_use_id: 'tool1', cc_session_id: 's1', question: 'First question', options: [{ id: 'a', label: 'A' }], created: '2026-08-28T07:19:21Z' } as StoredEvent],
    [249, { type: 'UserQuestionAnswered', tool_use_id: 'tool1', answer: { kind: 'Selected', option_id: 'a' }, created: '2026-08-28T07:19:21.010Z' } as StoredEvent],
    [250, { type: 'UserQuestionAsked', tool_use_id: 'tool2', cc_session_id: 's1', question: 'Second question', options: [{ id: 'b', label: 'B' }], created: '2026-08-28T07:19:21.020Z' } as StoredEvent],
  ]);
}

describe('a whole-second `created` orders by instant, not by string', () => {
  it('sorts the burst back into sequence order', () => {
    const sorted = sortEventsChronologically(twoQuestionTurn());
    expect(sorted.map(e => e.seq)).toEqual([246, 247, 248, 249, 250]);
  });

  it('keeps the first question ahead of the second', () => {
    const exchanges = groupIntoExchanges(twoQuestionTurn());
    expect(exchanges.map(e => e.userEvent.type)).toEqual([
      'MessageReceived',
      'UserQuestionAsked',
      'UserQuestionAsked',
    ]);
    const questions = exchanges.slice(1).map(e => (e.userEvent as { question?: string }).question);
    expect(questions).toEqual(['First question', 'Second question']);
  });

  it('applies the answer to the question it belongs to', () => {
    const exchanges = groupIntoExchanges(twoQuestionTurn());
    expect(findQuestionAnswer(exchanges[1], 'tool1')).toBeTruthy();
    expect(findQuestionAnswer(exchanges[2], 'tool2')).toBeFalsy();
  });
});
