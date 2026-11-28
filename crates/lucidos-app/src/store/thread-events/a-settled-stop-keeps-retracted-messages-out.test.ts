import { describe, it, expect } from 'vitest';
import { computeExchanges } from './exchange-grouping';
import type { StoredEvent } from './thread-event-types';
import { makeThreadState } from '../__tests__/thread-events-helpers';

const at = (second: number) => `2026-10-07T12:16:${String(second).padStart(2, '0')}Z`;

function stoppedThread(queued: string[], terminal: StoredEvent, streamed = true): Map<number, StoredEvent> {
  const rows: StoredEvent[] = [
    { type: 'MessageReceived', text: 'active', channel: 'chat', _eventId: 'active', created: at(1) },
    ...(streamed
      ? [{ type: 'TextStreamed', text: 'Still working...', request_event_id: 'active', created: at(2) } as StoredEvent]
      : []),
    ...queued.map((id, i): StoredEvent => (
      { type: 'MessageReceived', text: id, channel: 'chat', _eventId: id, created: at(3 + i) } as StoredEvent)),
    ...queued.map((id): StoredEvent => (
      { type: 'QueuedMessageRemoved', removed_message_id: id, channel: 'chat', created: at(10) } as StoredEvent)),
    terminal,
  ] as StoredEvent[];
  return new Map(rows.map((event, i) => [i + 1, event]));
}

const settled = { type: 'ResponseAborted', text: '', images: [], cause: 'stale_settle', created: at(11) } as StoredEvent;
const canceled = { type: 'ResponseCanceled', text: '', images: [], cause: 'user_stop', created: at(11) } as StoredEvent;

function messageIds(events: Map<number, StoredEvent>): (string | undefined)[] {
  return computeExchanges(makeThreadState(events))
    .filter(ex => ex.userEvent.type === 'MessageReceived')
    .map(ex => ex.userEvent._eventId);
}

function activeSteps(events: Map<number, StoredEvent>): string[] {
  const active = computeExchanges(makeThreadState(events)).find(ex => ex.userEvent._eventId === 'active');
  return active?.steps.map(s => s.event.type) ?? [];
}

describe('a Stop that ends the turn with no request id', () => {
  for (const [name, terminal] of [['a stale settle', settled], ['a cancel', canceled]] as const) {
    for (const streamed of [true, false]) {
      const when = streamed ? 'after the turn streamed' : 'before the turn wrote anything';
      it(`keeps one retracted follow-up out after ${name}, ${when}`, () => {
        expect(messageIds(stoppedThread(['q1'], terminal, streamed))).toEqual(['active']);
      });

      it(`keeps every retracted follow-up out after ${name}, ${when}`, () => {
        expect(messageIds(stoppedThread(['q1', 'q2'], terminal, streamed))).toEqual(['active']);
      });
    }
  }

  it('ends the running turn, which reads Aborted, whether or not it streamed', () => {
    expect(activeSteps(stoppedThread(['q1', 'q2'], settled))).toContain('ResponseAborted');
    expect(activeSteps(stoppedThread(['q1', 'q2'], settled, false))).toContain('ResponseAborted');
  });
});
