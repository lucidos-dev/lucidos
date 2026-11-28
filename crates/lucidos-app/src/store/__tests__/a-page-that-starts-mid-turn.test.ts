import { describe, it, expect } from 'vitest';
import { makeThreadState } from './thread-events-helpers';
import { computeExchanges, exchangeResponseEvents, queuedFollowupRun, renderOrderViolations, stampedEventIds, type StoredEvent, type ThreadState } from '../thread-events';


const T0 = Date.UTC(2026, 8, 20, 12, 0, 0);

function at(index: number): string {
  return new Date(T0 + index * 1000).toISOString();
}

function toolCall(index: number): StoredEvent {
  return {
    type: 'CodingAgentToolCalled',
    channel: 'claude_code',
    name: 'Bash',
    tool_use_id: `tool-${index}`,
    created: at(index),
    _eventId: `evt-${index}`,
  } as unknown as StoredEvent;
}

function userMessage(index: number): StoredEvent {
  return {
    type: 'MessageReceived',
    channel: 'claude_code',
    text: 'do the thing',
    created: at(index),
    _eventId: `evt-${index}`,
  } as unknown as StoredEvent;
}

function threadHolding(events: StoredEvent[], hasOlderEvents: boolean): ThreadState {
  const map = new Map<number, StoredEvent>();
  events.forEach((event, i) => map.set(i + 1, event));
  const thread = makeThreadState(map as Map<number, never>);
  thread.meta.channel = 'claude_code';
  thread.hasOlderEvents = hasOlderEvents;
  return thread;
}

function renderedRows(thread: ThreadState): number {
  return computeExchanges(thread)
    .reduce((n, exchange) => n + exchangeResponseEvents(exchange, false, true).length, 0);
}

describe('a page that starts mid-turn', () => {
  const midTurnPage = [toolCall(0), toolCall(1), toolCall(2), userMessage(3), toolCall(4), toolCall(5)];

  it('keeps the steps ahead of the page boundary', () => {
    const exchanges = computeExchanges(threadHolding(midTurnPage, true));

    expect(exchanges).toHaveLength(2);
    expect(exchanges[0].continuationFragment).toBe(true);
    expect(exchanges[0].steps.map(s => s.event._eventId)).toEqual(['evt-0', 'evt-1', 'evt-2']);
    expect(exchanges[1].userEvent.type).toBe('MessageReceived');
    expect(exchanges[1].steps.map(s => s.event._eventId)).toEqual(['evt-4', 'evt-5']);
  });

  it('draws a row for every loaded step', () => {
    expect(renderedRows(threadHolding(midTurnPage, true))).toBe(5);
  });

  it('opens no fragment once the page behind it has landed', () => {
    const whole = [userMessage(0), toolCall(1), toolCall(2), toolCall(3)];
    const exchanges = computeExchanges(threadHolding(whole, false));

    expect(exchanges).toHaveLength(1);
    expect(exchanges[0].continuationFragment).toBeUndefined();
    expect(exchanges[0].steps).toHaveLength(3);
  });

  it('holds a page carrying no boundary at all in one fragment', () => {
    const exchanges = computeExchanges(threadHolding([toolCall(0), toolCall(1), toolCall(2)], true));

    expect(exchanges).toHaveLength(1);
    expect(exchanges[0].continuationFragment).toBe(true);
    expect(exchanges[0].steps).toHaveLength(3);
  });

  it('still reports a thread served whole with no boundary as corrupt', () => {
    expect(computeExchanges(threadHolding([toolCall(0), toolCall(1)], false))).toHaveLength(0);
  });

  it('keys the fragment off its first step, so a backfill can re-point', () => {
    const exchanges = computeExchanges(threadHolding(midTurnPage, true));

    expect(exchanges[0].userEvent._eventId).toBe('evt-0');
  });

  it('reads in order, with its first step standing in for a boundary', () => {
    expect(renderOrderViolations(computeExchanges(threadHolding(midTurnPage, true)))).toEqual([]);
  });

  it('puts no deep-link id on a root with no starter', () => {
    const exchanges = computeExchanges(threadHolding(midTurnPage, true));

    expect(stampedEventIds(exchanges[0])).toEqual([]);
    expect(stampedEventIds(exchanges[1])).toEqual(['evt-3']);
  });
});

describe('a follow-up typed under a fragment', () => {
  const fragment = (steps: StoredEvent[]) => computeExchanges(threadHolding(steps, true))[0];

  it('queues behind the fragment rather than replacing it', () => {
    const thread = threadHolding([toolCall(0), toolCall(1)], true);
    thread.meta.channel = 'chat';
    thread.pendingUserMessages = [{ text: 'and also this', eventId: 'pending-1', created: at(9) }];
    const exchanges = computeExchanges(thread);

    expect(exchanges).toHaveLength(2);
    const run = queuedFollowupRun(exchanges, true);
    expect(run.activeIndex).toBe(0);
    expect(run.queuedOrder).toEqual([1]);
  });

  it('does not queue behind a fragment whose turn has ended', () => {
    const ended = {
      type: 'ResponseGenerated', text: 'done', created: at(2), _eventId: 'evt-2',
    } as unknown as StoredEvent;

    expect(queuedFollowupRun([fragment([toolCall(0), ended])], true).activeIndex).toBe(0);
    expect(queuedFollowupRun([fragment([toolCall(0), ended])], true).queuedOrder).toEqual([]);
  });
});
