/**
 * A message sent to a coding agent reads "Sent" until the agent takes it in,
 * then "Read". The engine records the read as `CodingAgentInputRead`, naming
 * the `MessageReceived` it acknowledges.
 */
import { describe, it, expect } from 'vitest';
import { groupIntoExchanges, readMarkers, type StoredEvent, type ThreadEvent } from '../thread-events';

function ev(seq: number, e: ThreadEvent, id?: string): readonly [number, StoredEvent] {
  const created = `2026-09-24T06:13:${String(seq).padStart(2, '0')}Z`;
  return [seq, { ...e, created, ...(id ? { _eventId: id } : {}) } as StoredEvent] as const;
}

const read = (id: string): ThreadEvent => ({ type: 'CodingAgentInputRead', input_event_id: id });
const CC = true;

describe('the read marker', () => {
  it('flips a message from Sent to Read when the agent reads it', () => {
    const sent = groupIntoExchanges(new Map([
      ev(1, { type: 'MessageReceived', text: 'start' }, 'm1'),
      ev(2, read('m1')),
      ev(3, { type: 'ResponseGenerated', text: 'ok' } as ThreadEvent),
      ev(4, { type: 'MessageReceived', text: 'also check the totals' }, 'm2'),
    ]));
    expect([...readMarkers(sent, CC)]).toEqual([[0, 'read'], [1, 'sent']]);

    const readNow = groupIntoExchanges(new Map([
      ev(1, { type: 'MessageReceived', text: 'start' }, 'm1'),
      ev(2, read('m1')),
      ev(3, { type: 'ResponseGenerated', text: 'ok' } as ThreadEvent),
      ev(4, { type: 'MessageReceived', text: 'also check the totals' }, 'm2'),
      ev(5, read('m2')),
    ]));
    expect(readMarkers(readNow, CC).get(1)).toBe('read');
  });

  it('never becomes a step of the turn it lands in', () => {
    const exchanges = groupIntoExchanges(new Map([
      ev(1, { type: 'MessageReceived', text: 'start' }, 'm1'),
      ev(2, read('m1')),
    ]));
    expect(exchanges).toHaveLength(1);
    expect(exchanges[0].steps.map(s => s.event.type)).not.toContain('CodingAgentInputRead');
  });

  it('marks nothing before the first read, so older history claims nothing', () => {
    const exchanges = groupIntoExchanges(new Map([
      ev(1, { type: 'MessageReceived', text: 'from before read events' }, 'm0'),
      ev(2, { type: 'ResponseGenerated', text: 'ok' } as ThreadEvent),
      ev(3, { type: 'MessageReceived', text: 'start' }, 'm1'),
      ev(4, read('m1')),
    ]));
    expect([...readMarkers(exchanges, CC)]).toEqual([[1, 'read']]);
  });

  it('keeps a message the agent never read at Sent', () => {
    const exchanges = groupIntoExchanges(new Map([
      ev(1, { type: 'MessageReceived', text: 'start' }, 'm1'),
      ev(2, read('m1')),
      ev(3, { type: 'MessageReceived', text: 'lost when the agent died', mode: 'agent' } as ThreadEvent, 'm2'),
      ev(4, { type: 'ResponseGenerated', text: 'done' } as ThreadEvent),
    ]));
    expect(readMarkers(exchanges, CC).get(1)).toBe('sent');
  });
});
