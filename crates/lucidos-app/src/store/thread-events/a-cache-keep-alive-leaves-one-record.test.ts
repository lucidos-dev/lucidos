/** A cache keep-alive resumes a waiting coding agent only to keep its prompt
 *  cache warm. Nothing of that turn renders: the waiting turn stays the last
 *  card, and a real delivery after it opens its own card as usual. The run of
 *  pings leaves one turnless card, which later status events update in place. */
import { describe, expect, it } from 'vitest';
import { computeExchanges, groupIntoExchanges, handleEvent, isTurnlessBoundary, type StoredEvent, type ThreadEvent, type ThreadMeta, type ThreadState } from '../thread-events';
import { awaitedQuestionCard } from './exchange-grouping';
import { exchangeResponseEvents } from './exchange-render';
import { applyAggregateToMeta, type ThreadAggregate } from './thread-meta';

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

const warming = (beats: number, minute: number) => ({
  type: 'CacheKeepAliveStatusChanged',
  created: at(minute),
  status: {
    state: 'warming', since: at(0), beats, budget_beats: 25, beat_every_secs: 240,
    cost_usd: beats * 0.02, context_tokens: 262_000, give_up_at: at(9),
  },
}) as unknown as StoredEvent;

const stopped = (minute: number) => ({
  type: 'CacheKeepAliveStatusChanged',
  created: at(minute),
  status: { state: 'stopped', at: at(minute), beats: 25, cost_usd: 0.64, context_tokens: 262_000 },
}) as unknown as StoredEvent;

const ended = (minute: number, beats = 3) => ({
  type: 'CacheKeepAliveStatusChanged',
  created: at(minute),
  status: { state: 'ended', since: at(0), at: at(minute), beats, cost_usd: beats * 0.02, saved_usd: 0.57 },
}) as unknown as StoredEvent;

const turnedOff = (minute: number, beats = 2) => ({
  type: 'CacheKeepAliveStatusChanged',
  created: at(minute),
  status: { state: 'turned_off', at: at(minute), beats, cost_usd: beats * 0.02, context_tokens: 262_000 },
}) as unknown as StoredEvent;

const cleared = (minute: number) => ({ type: 'CacheKeepAliveStatusChanged', created: at(minute) }) as unknown as StoredEvent;

const notKept = (minute: number) => ({
  type: 'CacheKeepAliveStatusChanged',
  created: at(minute),
  status: { state: 'not_kept', reason: 'no_wake_children' },
}) as unknown as StoredEvent;

/** The waiting turn, then `statuses` in order, one seq each. */
function withStatuses(...statuses: StoredEvent[]): Map<number, StoredEvent> {
  const events = waitingThenKeptWarm();
  statuses.forEach((status, i) => events.set(8 + i, status));
  return events;
}

const cards = (events: Map<number, StoredEvent>) =>
  groupIntoExchanges(events).filter((ex) => ex.userEvent.type === 'CacheKeepAliveStatusChanged');

describe('a cache keep-alive run', () => {
  it('draws one turnless card, however many statuses it emits', () => {
    const exchanges = groupIntoExchanges(withStatuses(warming(1, 6), warming(2, 7), warming(3, 8), stopped(9)));
    expect(exchanges).toHaveLength(2);
    expect(isTurnlessBoundary(exchanges[1].userEvent)).toBe(true);
    expect(exchanges[1].steps).toHaveLength(0);
    expect((exchanges[1].userEvent as { status: { state: string } }).status.state).toBe('stopped');
  });

  it('keeps the card once the engine clears the status and a delivery lands', () => {
    const events = withStatuses(warming(2, 6), cleared(7));
    events.set(10, {
      type: 'PromptInjected', text: 'An event you subscribed to has arrived', mode: 'agent', created: at(8), _eventId: 'd1',
      origin: { kind: 'engine', reason: { kind: 'event_wait', outcome: 'delivered', watched: ['BackgroundBashCompleted'], wait_reason: 'the suite' } },
    } as StoredEvent);
    events.set(11, { type: 'CodingAgentTextStreamed', text: 'the suite passed', created: at(8), request_event_id: 'd1', channel: 'claude_code' } as StoredEvent);
    const exchanges = groupIntoExchanges(events);
    expect(exchanges.map((ex) => ex.userEvent.type)).toEqual(['MessageReceived', 'CacheKeepAliveStatusChanged', 'PromptInjected']);
    expect((exchanges[1].userEvent as { status: { beats: number } }).status.beats).toBe(2);
    expect(exchanges[2].steps.map(({ event }) => (event as { text?: string }).text)).toContain('the suite passed');
  });

  it('keeps the card when a delivery lands before the engine clears the status', () => {
    const events = withStatuses(warming(2, 6));
    events.set(9, {
      type: 'PromptInjected', text: 'An event you subscribed to has arrived', mode: 'agent', created: at(7), _eventId: 'd1',
      origin: { kind: 'engine', reason: { kind: 'event_wait', outcome: 'delivered', watched: ['BackgroundBashCompleted'], wait_reason: 'the suite' } },
    } as StoredEvent);
    events.set(10, cleared(8));
    const exchanges = groupIntoExchanges(events);
    expect(exchanges.map((ex) => ex.userEvent.type)).toEqual(['MessageReceived', 'CacheKeepAliveStatusChanged', 'PromptInjected']);
  });

  it('fills the card in with how the run ended, then closes the run', () => {
    const [card, ...rest] = cards(withStatuses(warming(3, 6), ended(8), warming(1, 9)));
    expect((card.userEvent as { status: { state: string } }).status.state).toBe('ended');
    expect(rest).toHaveLength(1);
    expect((rest[0].userEvent as { status: { state: string } }).status.state).toBe('warming');
  });

  it('keeps one card through a switch off and back on', () => {
    const [card, ...rest] = cards(withStatuses(warming(2, 6), turnedOff(7), warming(3, 8)));
    expect(rest).toHaveLength(0);
    expect((card.userEvent as { status: { state: string; beats: number } }).status).toMatchObject({ state: 'warming', beats: 3 });
  });

  it('shows a run turned off as its card, until the engine clears it', () => {
    const [card] = cards(withStatuses(warming(2, 6), turnedOff(7)));
    expect((card.userEvent as { status: { state: string } }).status.state).toBe('turned_off');
  });

  it('opens a new card for the next run', () => {
    expect(cards(withStatuses(warming(1, 6), cleared(7), warming(1, 8)))).toHaveLength(2);
  });

  it('opens a new card when the pings restart without a clear between', () => {
    expect(cards(withStatuses(warming(3, 6), warming(1, 7)))).toHaveLength(2);
  });

  it('draws its card from the first status, before any ping', () => {
    const [card, ...rest] = cards(withStatuses(warming(0, 6)));
    expect(rest).toHaveLength(0);
    expect((card.userEvent as { status: { beats: number } }).status.beats).toBe(0);
  });

  it('keeps the card of a run through its first ping', () => {
    const [card, ...rest] = cards(withStatuses(warming(0, 6), warming(1, 7)));
    expect(rest).toHaveLength(0);
    expect((card.userEvent as { status: { beats: number } }).status.beats).toBe(1);
  });

  it('closes the card of a wait that ended before its first ping', () => {
    const [card, ...rest] = cards(withStatuses(warming(0, 6), ended(7, 0)));
    expect(rest).toHaveLength(0);
    expect((card.userEvent as { status: { state: string; beats: number } }).status).toMatchObject({ state: 'ended', beats: 0 });
  });

  it('keeps the card of a run turned off before its first ping, and turns it back on in place', () => {
    const [card, ...rest] = cards(withStatuses(warming(0, 6), turnedOff(7, 0), warming(0, 8)));
    expect(rest).toHaveLength(0);
    expect((card.userEvent as { status: { state: string } }).status.state).toBe('warming');
  });

  it('draws no card for a thread the engine does not keep warm', () => {
    expect(cards(withStatuses(notKept(6)))).toHaveLength(0);
  });

  it('keeps the run through not_kept, since the same wait may warm again', () => {
    const [card, ...rest] = cards(withStatuses(warming(2, 6), notKept(7), warming(3, 8)));
    expect(rest).toHaveLength(0);
    expect((card.userEvent as { status: { beats: number } }).status.beats).toBe(3);
  });

  it('opens a new card for another wait that reaches the same ping count', () => {
    expect(cards(withStatuses(warming(1, 6), sinceAt(warming(1, 8), 7)))).toHaveLength(2);
  });

  it('opens a new card when a stopped wait is followed by a warming one', () => {
    expect(cards(withStatuses(warming(3, 6), stopped(7), warming(3, 8)))).toHaveLength(2);
  });

  it('keeps the card key through updates, so the node is not remounted', () => {
    const withId = (event: StoredEvent, id: string) => ({ ...event, _eventId: id }) as StoredEvent;
    const [card] = cards(withStatuses(withId(warming(1, 6), 's1'), withId(warming(2, 7), 's2')));
    expect(card.userEvent._eventId).toBe('s1');
    expect((card.userEvent as { status: { beats: number } }).status.beats).toBe(2);
  });

  it('updates the card in place on the incremental path', () => {
    const events = withStatuses(warming(1, 6));
    const thread = { meta: { updatedAt: at(1) }, events, streamingBuffer: '', pendingUserMessages: [] } as unknown as ThreadState;
    const before = computeExchanges(thread);
    const card = before[1];
    handleEvent(new Map([['t1', thread]]), 't1', 9, warming(2, 7) as unknown as ThreadEvent, at(7));
    const after = computeExchanges(thread);
    expect(after).toHaveLength(2);
    expect(after[1]).toBe(card);
    expect((after[1].userEvent as { status: { beats: number } }).status.beats).toBe(2);
  });

  it('moves no recency', () => {
    const thread = {
      meta: { updatedAt: at(1) },
      events: new Map(),
      streamingBuffer: '',
      pendingUserMessages: [],
    } as unknown as ThreadState;
    handleEvent(new Map([['t1', thread]]), 't1', 1, warming(1, 9) as unknown as ThreadEvent, at(9));
    expect(thread.meta.updatedAt).toBe(at(1));
  });
});

describe('the thread aggregate', () => {
  it('carries the status into meta, and a repeat changes nothing', () => {
    const meta = { summaryVersion: 0, cacheKeepAliveStatus: null } as unknown as ThreadMeta;
    const status = { state: 'not_kept', reason: 'no_wake_children' } as const;
    const agg = { summaryVersion: 1, cacheKeepAliveStatus: status } as unknown as ThreadAggregate;
    expect(applyAggregateToMeta(meta, agg)).toBe(true);
    expect(meta.cacheKeepAliveStatus).toEqual(status);
    expect(applyAggregateToMeta(meta, { ...agg, cacheKeepAliveStatus: { ...status } })).toBe(false);
  });
});

/** A coding agent parked on a question card, which the user has yet to answer. */
function parkedOnAQuestion(): Map<number, StoredEvent> {
  const events = new Map<number, StoredEvent>();
  events.set(1, { type: 'MessageReceived', text: 'run the suite', mode: 'human', created: at(0), _eventId: 'm1', channel: 'claude_code' } as StoredEvent);
  events.set(2, { type: 'UserQuestionAsked', tool_use_id: 'q1', questions: [], created: at(1), request_event_id: 'm1', channel: 'claude_code' } as unknown as StoredEvent);
  return events;
}

describe('a keep-alive run while the turn waits on a question card', () => {
  const answered = (events: Map<number, StoredEvent>, seq: number) => {
    events.set(seq, { type: 'UserQuestionAnswered', tool_use_id: 'q1', answers: [], created: at(7), request_event_id: 'm1', channel: 'claude_code' } as unknown as StoredEvent);
    events.set(seq + 1, { type: 'CodingAgentTextStreamed', text: 'carrying on', created: at(7), request_event_id: 'm1', channel: 'claude_code' } as StoredEvent);
  };
  const rows = (events: Map<number, StoredEvent>) =>
    exchangeResponseEvents(groupIntoExchanges(events)[1]).filter((row) => row.type === 'cache_keep_alive');

  it('opens no exchange, and the question card stays the one waited on', () => {
    const events = parkedOnAQuestion();
    events.set(3, warming(1, 5));
    const exchanges = groupIntoExchanges(events);
    expect(exchanges.map((ex) => ex.userEvent.type)).toEqual(['MessageReceived', 'UserQuestionAsked']);
    expect(awaitedQuestionCard(exchanges)).toBe('q1');
  });

  it('draws its card under the question, and updates it in place', () => {
    const events = parkedOnAQuestion();
    events.set(3, warming(1, 5));
    events.set(4, warming(2, 6));
    const [row, ...rest] = rows(events);
    expect(rest).toHaveLength(0);
    expect((row as { status: { beats: number } }).status.beats).toBe(2);
  });

  it('keeps the card in the turn once the answer lands, and fills it in as the run ends', () => {
    const events = parkedOnAQuestion();
    events.set(3, warming(2, 5));
    answered(events, 4);
    events.set(6, ended(8, 2));
    const exchanges = groupIntoExchanges(events);
    expect(exchanges).toHaveLength(2);
    const [row, ...rest] = rows(events);
    expect(rest).toHaveLength(0);
    expect((row as { status: { state: string } }).status.state).toBe('ended');
  });

  it('leaves a later run of the same thread to a card of its own', () => {
    const events = parkedOnAQuestion();
    events.set(3, warming(2, 5));
    answered(events, 4);
    events.set(6, ended(8, 2));
    events.set(7, warming(1, 9));
    const exchanges = groupIntoExchanges(events);
    expect(exchanges.map((ex) => ex.userEvent.type)).toEqual(['MessageReceived', 'UserQuestionAsked', 'CacheKeepAliveStatusChanged']);
  });

  it('draws the card before the first ping', () => {
    const events = parkedOnAQuestion();
    events.set(3, warming(0, 5));
    expect(groupIntoExchanges(events)).toHaveLength(2);
    expect(rows(events)).toHaveLength(1);
  });

  it('puts the card in the same place on the incremental path', () => {
    const events = parkedOnAQuestion();
    events.set(3, warming(1, 5));
    const thread = { meta: { updatedAt: at(1) }, events, streamingBuffer: '', pendingUserMessages: [] } as unknown as ThreadState;
    computeExchanges(thread);
    handleEvent(new Map([['t1', thread]]), 't1', 4, warming(2, 6) as unknown as ThreadEvent, at(6));
    const after = computeExchanges(thread);
    expect(after).toHaveLength(2);
    const [row] = exchangeResponseEvents(after[1]).filter((r) => r.type === 'cache_keep_alive');
    expect((row as { status: { beats: number } }).status.beats).toBe(2);
  });

  it('gives each run under the question a row of its own', () => {
    const events = parkedOnAQuestion();
    events.set(3, warming(2, 5));
    events.set(4, cleared(6));
    events.set(5, sinceAt(warming(1, 8), 7));
    const beats = rows(events).map((row) => (row as { status: { beats: number } }).status.beats);
    expect(beats).toEqual([2, 1]);
  });

  it('lands under the question behind a held callback', () => {
    const events = parkedOnAQuestion();
    events.set(3, {
      type: 'PromptInjected', text: 'An event you subscribed to has arrived', mode: 'agent', created: at(4), _eventId: 'd1',
      origin: { kind: 'engine', reason: { kind: 'event_wait', outcome: 'delivered', watched: ['BackgroundBashCompleted'], wait_reason: 'the suite' } },
    } as StoredEvent);
    events.set(4, warming(1, 5));
    expect(rows(events)).toHaveLength(1);
    expect(cards(events)).toHaveLength(0);
  });

  it('never lands under an MCP consent card', () => {
    const events = new Map<number, StoredEvent>();
    events.set(1, { type: 'MessageReceived', text: 'read the file', mode: 'human', created: at(0), _eventId: 'm1', channel: 'claude_code' } as StoredEvent);
    events.set(2, { type: 'McpConsentRequested', tool: 'fs.read', args: {}, created: at(1), request_event_id: 'm1' } as unknown as StoredEvent);
    events.set(3, warming(1, 5));
    const exchanges = groupIntoExchanges(events);
    expect(exchanges.find((ex) => ex.userEvent.type === 'McpConsentRequested')?.steps).toEqual([]);
    expect(cards(events)).toHaveLength(1);
  });
});

/** `event` reporting a wait that began at `minute`. */
function sinceAt(event: StoredEvent, minute: number): StoredEvent {
  const { status } = event as unknown as { status: object };
  return { ...event, status: { ...status, since: at(minute) } } as unknown as StoredEvent;
}

describe('a keep-alive run that ended', () => {
  it('reopens its card when the same wait warms again', () => {
    const [card, ...rest] = cards(withStatuses(warming(3, 6), ended(7), warming(4, 8)));
    expect(rest).toHaveLength(0);
    expect((card.userEvent as { status: { state: string; beats: number } }).status).toMatchObject({ state: 'warming', beats: 4 });
  });

  it('opens a new card for another wait', () => {
    expect(cards(withStatuses(warming(3, 6), ended(7), sinceAt(warming(3, 8), 7)))).toHaveLength(2);
  });

  it('stays closed once the status clears', () => {
    expect(cards(withStatuses(warming(3, 6), ended(7), cleared(8), warming(4, 9)))).toHaveLength(2);
  });
});
