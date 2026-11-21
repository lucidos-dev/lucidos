import { describe, it, expect } from 'vitest';
import { makeThreadState } from './thread-events-helpers';
import { computeExchanges, exchangeResponseEvents, queuedFollowupRun, renderOrderViolations, stampedEventIds, type StoredEvent, type ThreadState } from '../thread-events';

/** A long thread opens on its newest page, and that page routinely starts in
 *  the middle of a turn. The fold used to drop every step ahead of the page's
 *  first boundary. One reported thread drew 28 rows out of 400 loaded events,
 *  and a whole second page added none.
 *
 *  The transcript then sat shorter than the pane, and a transcript that does
 *  not overflow emits no scroll event. The scroll event is the only thing that
 *  fetches older history, so the thread was stuck with no way forward.
 *
 *  Measurements and the rest of the chain:
 *  docs/plans/2026-09-20-a-paged-transcript-the-reader-can-reach.md. */

const T0 = Date.UTC(2026, 8, 20, 12, 0, 0);

/** One event, stamped a second after the last. */
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

/** A thread holding `events`, paged or served whole. */
function threadHolding(events: StoredEvent[], hasOlderEvents: boolean): ThreadState {
  const map = new Map<number, StoredEvent>();
  events.forEach((event, i) => map.set(i + 1, event));
  const thread = makeThreadState(map as Map<number, never>);
  thread.meta.channel = 'claude_code';
  thread.hasOlderEvents = hasOlderEvents;
  return thread;
}

/** Every row the transcript would draw, across every exchange. */
function renderedRows(thread: ThreadState): number {
  return computeExchanges(thread)
    .reduce((n, exchange) => n + exchangeResponseEvents(exchange, false, true).length, 0);
}

describe('a page that starts mid-turn', () => {
  // Three orphan steps, then a boundary, then two more. The reported shape.
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
    // Same events, nothing older: the boundary that opens the turn is loaded.
    const whole = [userMessage(0), toolCall(1), toolCall(2), toolCall(3)];
    const exchanges = computeExchanges(threadHolding(whole, false));

    expect(exchanges).toHaveLength(1);
    expect(exchanges[0].continuationFragment).toBeUndefined();
    expect(exchanges[0].steps).toHaveLength(3);
  });

  it('holds a page carrying no boundary at all in one fragment', () => {
    // The newest page of a long coding-agent thread can sit wholly inside one
    // running turn. That folded to ZERO exchanges, and the transcript then
    // reported the thread corrupt.
    const exchanges = computeExchanges(threadHolding([toolCall(0), toolCall(1), toolCall(2)], true));

    expect(exchanges).toHaveLength(1);
    expect(exchanges[0].continuationFragment).toBe(true);
    expect(exchanges[0].steps).toHaveLength(3);
  });

  it('still reports a thread served whole with no boundary as corrupt', () => {
    // Nothing older explains the missing boundary, so these events really are
    // broken. The corrupt state and its rebuild affordance must stay reachable.
    expect(computeExchanges(threadHolding([toolCall(0), toolCall(1)], false))).toHaveLength(0);
  });

  it('keys the fragment off its first step, so a backfill can re-point', () => {
    const exchanges = computeExchanges(threadHolding(midTurnPage, true));

    expect(exchanges[0].userEvent._eventId).toBe('evt-0');
  });

  /** That first step is the fragment's `userEvent` AND its first row, so the
   *  order check sees one event twice. Both copies carry the same key, so
   *  neither can read as late. Asserted rather than reasoned about. */
  it('reads in order, with its first step standing in for a boundary', () => {
    expect(renderOrderViolations(computeExchanges(threadHolding(midTurnPage, true)))).toEqual([]);
  });

  /** A step is never addressable on its own, so the fragment's root wears no
   *  id. Stamping one would also collide with the failure card when the step
   *  standing in for the boundary IS a `ResponseFailed`. */
  it('puts no deep-link id on a root with no starter', () => {
    const exchanges = computeExchanges(threadHolding(midTurnPage, true));

    expect(stampedEventIds(exchanges[0])).toEqual([]);
    expect(stampedEventIds(exchanges[1])).toEqual(['evt-3']);
  });
});

/** A fragment holds a live turn, and the follow-up machinery decides that from
 *  `userEvent.type`. A fragment's is a step type, so it read as holding none:
 *  the reader's own message became the active turn, took the running turn's
 *  stream into its bubble, and lost its Queued tag and its retract control. */
describe('a follow-up typed under a fragment', () => {
  const fragment = (steps: StoredEvent[]) => computeExchanges(threadHolding(steps, true))[0];

  it('queues behind the fragment rather than replacing it', () => {
    const thread = threadHolding([toolCall(0), toolCall(1)], true);
    thread.meta.channel = 'chat';
    thread.pendingUserMessages = [{ text: 'and also this', eventId: 'pending-1', created: at(9) }];
    const exchanges = computeExchanges(thread);

    // Index 0 is the fragment, 1 the optimistic message.
    expect(exchanges).toHaveLength(2);
    const run = queuedFollowupRun(exchanges, true);
    expect(run.activeIndex).toBe(0);
    expect(run.queuedOrder).toEqual([1]);
  });

  /** The terminal check runs first and still decides. A fragment whose page
   *  carries the turn's end is finished, so the next message IS the turn. */
  it('does not queue behind a fragment whose turn has ended', () => {
    const ended = {
      type: 'ResponseGenerated', text: 'done', created: at(2), _eventId: 'evt-2',
    } as unknown as StoredEvent;

    expect(queuedFollowupRun([fragment([toolCall(0), ended])], true).activeIndex).toBe(0);
    expect(queuedFollowupRun([fragment([toolCall(0), ended])], true).queuedOrder).toEqual([]);
  });
});
