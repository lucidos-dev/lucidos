import { describe, it, expect } from 'vitest';
import { locateReadingTarget, readingTargetOf } from '../ThreadView';
import { makeThreadState } from '../../../store/__tests__/thread-events-helpers';
import { computeExchanges, type StoredEvent, type ThreadState } from '../../../store/thread-events';

/** Where the window must walk to put a reader back on their row.
 *
 *  A row is found by the event id of its tool call, among a turn's events.
 *  That id survives what the turn's own id does not: a page boundary that cuts
 *  the turn into a *continuation fragment*, which carries no turn id at all.
 *  Plan: docs/plans/2026-09-23-the-reading-position-and-the-thumb-hold-still.md */

const T0 = Date.UTC(2026, 8, 23, 12, 0, 0);
const at = (i: number) => new Date(T0 + i * 1000).toISOString();

function toolCall(i: number): StoredEvent {
  return {
    type: 'CodingAgentToolCalled', channel: 'claude_code', name: 'Bash',
    tool_use_id: `tool-${i}`, description: `Run ${i}`, created: at(i), _eventId: `evt-${i}`,
  } as unknown as StoredEvent;
}

function userMessage(i: number): StoredEvent {
  return {
    type: 'MessageReceived', channel: 'claude_code', text: 'go', created: at(i), _eventId: `evt-${i}`,
  } as unknown as StoredEvent;
}

function exchangesOf(events: StoredEvent[], paged: boolean) {
  const map = new Map<number, StoredEvent>();
  events.forEach((event, i) => map.set(i + 1, event));
  const thread: ThreadState = makeThreadState(map as Map<number, never>);
  thread.meta.channel = 'claude_code';
  thread.hasOlderEvents = paged;
  return computeExchanges(thread);
}

describe('readingTargetOf', () => {
  it('names a turn for a turn record and a row for a row record', () => {
    expect(readingTargetOf({ kind: 'anchor', eventId: 't', relTop: 0 })).toEqual({ kind: 'turn', id: 't' });
    expect(readingTargetOf({ kind: 'row', rowEventId: 'r', relTop: 0 })).toEqual({ kind: 'row', id: 'r' });
  });

  it('names nothing for a record that is a place without content', () => {
    expect(readingTargetOf({ kind: 'offset', top: 10 })).toBeNull();
    expect(readingTargetOf({ kind: 'live-edge' })).toBeNull();
    expect(readingTargetOf(null)).toBeNull();
  });
});

describe('locateReadingTarget', () => {
  const whole = [userMessage(0), toolCall(1), toolCall(2), toolCall(3), userMessage(4), toolCall(5)];

  it('finds a row inside a whole turn, at its index among the drawn rows', () => {
    const exchanges = exchangesOf(whole, false);
    expect(locateReadingTarget(exchanges, { kind: 'row', id: 'evt-3' })).toEqual({ index: 0, row: 2 });
    expect(locateReadingTarget(exchanges, { kind: 'row', id: 'evt-5' })).toEqual({ index: 1, row: 0 });
  });

  it('finds a row inside a continuation fragment, which has no turn id', () => {
    // The page starts mid-turn: the fragment holds evt-1..evt-3 and no opening
    // message. A turn anchor could not name it. Its rows are still found.
    const exchanges = exchangesOf(whole.slice(1), true);
    expect(exchanges[0].continuationFragment).toBe(true);
    expect(locateReadingTarget(exchanges, { kind: 'row', id: 'evt-2' })).toEqual({ index: 0, row: 1 });
  });

  it('finds a turn by its opening message', () => {
    const exchanges = exchangesOf(whole, false);
    expect(locateReadingTarget(exchanges, { kind: 'turn', id: 'evt-4' })).toEqual({ index: 1, row: 0 });
  });

  it('answers -1 for a row behind the loaded page', () => {
    const exchanges = exchangesOf(whole.slice(4), true);
    expect(locateReadingTarget(exchanges, { kind: 'row', id: 'evt-2' }).index).toBe(-1);
  });
});
