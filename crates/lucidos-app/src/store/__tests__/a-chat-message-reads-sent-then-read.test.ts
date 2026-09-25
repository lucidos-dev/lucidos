/**
 * A message sent to the Lucidos Agent reads "Sent" until the loop takes it,
 * then "Read", the same label a coding-agent thread shows. The loop takes a
 * message by starting its turn, or by injecting it into a running one.
 */
import { describe, it, expect } from 'vitest';
import { groupIntoExchanges, queuedFollowupRun, readMarkers, type StoredEvent, type ThreadEvent } from '../thread-events';

function ev(seq: number, e: ThreadEvent, id?: string): readonly [number, StoredEvent] {
  const created = `2026-09-24T06:13:${String(seq).padStart(2, '0')}Z`;
  return [seq, { ...e, created, ...(id ? { _eventId: id } : {}) } as StoredEvent] as const;
}

const CHAT = false;

describe('the read marker on a chat thread', () => {
  it('reads Sent until the turn starts, then Read', () => {
    const sent = groupIntoExchanges(new Map([
      ev(1, { type: 'MessageReceived', text: 'why is it top level?' }, 'm1'),
    ]));
    expect(readMarkers(sent, CHAT).get(0)).toBe('sent');

    const read = groupIntoExchanges(new Map([
      ev(1, { type: 'MessageReceived', text: 'why is it top level?' }, 'm1'),
      ev(2, { type: 'ResponseGenerated', text: 'because' } as ThreadEvent),
    ]));
    expect(readMarkers(read, CHAT).get(0)).toBe('read');
  });

  it('reads Read once a follow-up is injected into the running turn', () => {
    const exchanges = groupIntoExchanges(new Map([
      ev(1, { type: 'MessageReceived', text: 'start' }, 'm1'),
      ev(2, { type: 'MessageReceived', text: 'also check the totals' }, 'm2'),
      ev(3, { type: 'UserPromptInjected', text: 'also check the totals', mode: 'human', injected_message_id: 'm2' } as ThreadEvent),
    ]));
    const markers = readMarkers(exchanges, CHAT);
    const m2 = exchanges.findIndex(ex => ex.userEvent._eventId === 'm2');
    expect(markers.get(m2)).toBe('read');
  });

  it('keeps a queued follow-up at Queued while the running turn writes its to-do list', () => {
    // `TodoListWritten` carries no request id, so nothing routes it by turn.
    const exchanges = groupIntoExchanges(new Map([
      ev(1, { type: 'MessageReceived', text: 'start' }, 'm1'),
      ev(2, { type: 'ToolCalled', tool_name: 'todo_write', args: {}, request_event_id: 'm1' } as unknown as ThreadEvent, 't1'),
      ev(3, { type: 'MessageReceived', text: 'also check the totals' }, 'm2'),
      ev(4, { type: 'TodoListWritten', items: [] } as unknown as ThreadEvent),
    ]));
    expect(exchanges[0].steps.map(s => s.event.type)).toContain('TodoListWritten');
    expect(exchanges[1].steps).toHaveLength(0);
    expect(queuedFollowupRun(exchanges, true).queuedOrder).toEqual([1]);
    expect(readMarkers(exchanges, CHAT).get(1)).toBe('sent');
  });

  it('marks nothing a caller said on a call', () => {
    const exchanges = groupIntoExchanges(new Map([
      ev(1, { type: 'MessageReceived', text: 'hello', voice_session_id: 'v1' } as ThreadEvent, 'm1'),
      ev(2, { type: 'ResponseGenerated', text: 'hi' } as ThreadEvent),
    ]));
    expect(readMarkers(exchanges, CHAT).size).toBe(0);
  });
});
