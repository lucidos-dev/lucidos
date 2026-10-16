/** A cache keep-alive resumes a waiting coding agent only to keep its prompt
 *  cache warm. Nothing of that turn renders: the waiting turn stays the last
 *  card, and a real delivery after it opens its own card as usual. */
import { describe, expect, it } from 'vitest';
import { groupIntoExchanges, handleEvent, type StoredEvent, type ThreadEvent, type ThreadState } from '../thread-events';

const at = (minute: number) => `2026-10-03T14:0${minute}:00.000Z`;

function waitingThenKeptWarm(): Map<number, StoredEvent> {
  const events = new Map<number, StoredEvent>();
  events.set(1, { type: 'MessageReceived', text: 'run the suite', mode: 'human', created: at(0), _eventId: 'm1', channel: 'claude_code' } as StoredEvent);
  events.set(2, { type: 'CodingAgentTextStreamed', text: 'started it, waiting', created: at(1), request_event_id: 'm1', channel: 'claude_code' } as StoredEvent);
  events.set(3, { type: 'CodingAgentIdled', created: at(1), request_event_id: 'm1', channel: 'claude_code' } as StoredEvent);
  events.set(4, {
    type: 'PromptInjected', text: 'Cache keep-alive', mode: 'engine', created: at(5), _eventId: 'k1',
    origin: { kind: 'engine', reason: { kind: 'cache_keep_alive' } },
  } as StoredEvent);
  events.set(5, { type: 'CodingAgentTextStreamed', text: 'waiting', created: at(5), request_event_id: 'k1', channel: 'claude_code' } as StoredEvent);
  events.set(6, { type: 'ResponseGenerated', text: 'waiting', created: at(5), request_event_id: 'k1', channel: 'claude_code' } as StoredEvent);
  events.set(7, { type: 'CodingAgentIdled', reason: 'cache_keep_alive', created: at(5), request_event_id: 'k1', channel: 'claude_code' } as StoredEvent);
  return events;
}

describe('a cache keep-alive', () => {
  it('draws no card, and leaves the waiting turn last', () => {
    const exchanges = groupIntoExchanges(waitingThenKeptWarm());
    expect(exchanges).toHaveLength(1);
    const texts = exchanges[0].steps.map(({ event }) => (event as { text?: string }).text);
    expect(texts).not.toContain('waiting');
  });

  it('lets a real delivery after it open a card of its own', () => {
    const events = waitingThenKeptWarm();
    events.set(8, {
      type: 'PromptInjected', text: 'An event you subscribed to has arrived', mode: 'agent', created: at(8), _eventId: 'd1',
      origin: { kind: 'engine', reason: { kind: 'event_wait', outcome: 'delivered', watched: ['BackgroundBashCompleted'], wait_reason: 'the suite' } },
    } as StoredEvent);
    events.set(9, { type: 'CodingAgentTextStreamed', text: 'the suite passed', created: at(8), request_event_id: 'd1', channel: 'claude_code' } as StoredEvent);
    const exchanges = groupIntoExchanges(events);
    expect(exchanges).toHaveLength(2);
    expect(exchanges[1].steps.map(({ event }) => (event as { text?: string }).text)).toContain('the suite passed');
  });
});

describe('a card raised in a keep-alive turn', () => {
  it('still renders, so the thread waiting on it can be answered', () => {
    const events = waitingThenKeptWarm();
    events.set(8, { type: 'UserQuestionAsked', tool_use_id: 'q1', questions: [], created: at(6), request_event_id: 'k1', channel: 'claude_code' } as unknown as StoredEvent);
    events.set(9, { type: 'CodingAgentTextStreamed', text: 'after the answer', created: at(7), request_event_id: 'k1', channel: 'claude_code' } as StoredEvent);
    const exchanges = groupIntoExchanges(events);
    const types = exchanges.flatMap((ex) => [ex.userEvent.type, ...ex.steps.map(({ event }) => event.type)]);
    expect(types).toContain('UserQuestionAsked');
    const texts = exchanges.flatMap((ex) => ex.steps.map(({ event }) => (event as { text?: string }).text));
    expect(texts).toContain('after the answer');
  });
});

describe('a cache keep-alive on a live thread', () => {
  it('moves no recency, while a real turn still does', () => {
    const thread = {
      meta: { updatedAt: at(1) },
      events: new Map(),
      streamingBuffer: '',
      pendingUserMessages: [],
    } as unknown as ThreadState;
    const map = new Map([['t1', thread]]);
    const anchor = {
      type: 'PromptInjected', text: 'Cache keep-alive', mode: 'engine',
      origin: { kind: 'engine', reason: { kind: 'cache_keep_alive' } },
    } as unknown as ThreadEvent;
    handleEvent(map, 't1', 1, anchor, at(5), 'k1');
    handleEvent(map, 't1', 2, { type: 'CodingAgentTextStreamed', text: 'waiting', request_event_id: 'k1' } as unknown as ThreadEvent, at(5));
    handleEvent(map, 't1', 3, { type: 'CodingAgentIdled', reason: 'cache_keep_alive', request_event_id: 'k1' } as unknown as ThreadEvent, at(5));
    expect(thread.meta.updatedAt).toBe(at(1));

    handleEvent(map, 't1', 4, { type: 'PromptInjected', text: 'arrived', mode: 'agent' } as unknown as ThreadEvent, at(8), 'd1');
    expect(thread.meta.updatedAt).toBe(at(8));
  });
});
