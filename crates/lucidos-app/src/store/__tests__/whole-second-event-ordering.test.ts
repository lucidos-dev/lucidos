/**
 * An event landing on a whole second keeps its place in the fold.
 *
 * The engine writes `created` with only the digits it needs. So one event in a
 * burst can arrive as `...:21Z` while its neighbours carry `...:21.010Z`. `.`
 * sorts before `Z`, so the fold's old lexical compare moved that event to the
 * END of the thread.
 *
 * The payload below is the real one, read off a Playwright trace of
 * `coding-agent-question.spec.ts` failing under `mobile-webkit`: the second
 * question rendered above the first, and the answer applied to neither.
 */
import { describe, it, expect } from 'vitest';
import { groupIntoExchanges, sortEventsChronologically, type StoredEvent } from '../thread-events';
import { findQuestionAnswer } from '../thread-events/exchange';

/** Five events, sequence 246 to 250, with 248 on a whole second. */
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
    // Folded out of order, the answer arrives before its own divider exists and
    // is dropped as an orphan, which is what left both panels asking.
    expect(findQuestionAnswer(exchanges[1], 'tool1')).toBeTruthy();
    expect(findQuestionAnswer(exchanges[2], 'tool2')).toBeFalsy();
  });
});
