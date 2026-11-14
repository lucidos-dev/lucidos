/** An unsent message never reached the engine, so no agent ever reads it. Its
 *  client-only pair holds its own failure card and nothing else, and a running
 *  turn keeps every row it writes afterwards. */
import { describe, expect, it } from 'vitest';
import { groupIntoExchanges, queuedFollowupRun, type StoredEvent } from '../thread-events';
import { renderOrderViolations } from './render-order';

const at = (minute: number) => `2026-10-03T14:0${minute}:00.000Z`;

function runningTurnWithUnsentMessage(): Map<number, StoredEvent> {
  const events = new Map<number, StoredEvent>();
  events.set(1, { type: 'MessageReceived', text: 'first', mode: 'human', created: at(0), _eventId: 'm1', channel: 'claude_code' } as StoredEvent);
  events.set(2, { type: 'CodingAgentPromptSent', text: 'first', created: at(0), request_event_id: 'm1', channel: 'claude_code' } as StoredEvent);
  events.set(3, { type: 'CodingAgentTextStreamed', text: 'working', created: at(1), request_event_id: 'm1', channel: 'claude_code' } as StoredEvent);
  // The pair `showUnsentExchange` draws: the message, its failure one seq above.
  events.set(-2, { type: 'MessageReceived', text: 'unsent one', created: at(2), _eventId: 'u1', _unsent: true } as StoredEvent);
  events.set(-1, { type: 'ResponseFailed', error: 'no answer', created: at(2), _unsent: true } as StoredEvent);
  events.set(4, { type: 'CodingAgentTextStreamed', text: 'more work', created: at(3), request_event_id: 'm1', channel: 'claude_code' } as StoredEvent);
  return events;
}

const texts = (steps: { event: StoredEvent }[]) =>
  steps.map(({ event }) => `${event.type}:${(event as { text?: string }).text ?? ''}`);

describe('an unsent message', () => {
  it('leaves the running turn its later output', () => {
    const [turn] = groupIntoExchanges(runningTurnWithUnsentMessage());
    expect(texts(turn.steps)).toEqual([
      'CodingAgentPromptSent:first',
      'CodingAgentTextStreamed:working',
      'CodingAgentTextStreamed:more work',
    ]);
  });

  it('never becomes the live turn, so the running one keeps its stream and status', () => {
    const exchanges = groupIntoExchanges(runningTurnWithUnsentMessage());
    for (const threadIsCC of [true, false]) {
      expect(queuedFollowupRun(exchanges, true, threadIsCC).activeIndex).toBe(0);
    }
    expect(queuedFollowupRun(exchanges, false).activeIndex).toBe(0);
  });

  it('is a named render-order exception, since the turn above writes past it', () => {
    expect(renderOrderViolations(groupIntoExchanges(runningTurnWithUnsentMessage()))).toEqual([]);
  });

  it('holds only its own failure card', () => {
    const unsent = groupIntoExchanges(runningTurnWithUnsentMessage()).find((ex) => ex.userSeq === -2);
    expect(texts(unsent!.steps)).toEqual(['ResponseFailed:']);
  });
});
