import { describe, it, expect, beforeEach } from 'vitest';
import { getExchanges, insertEvents, makeThread, resetSeqCounter } from './thread-flows-helpers';
import {
  BOUNDARY_CONTINUATION_HANDOFF,
  EXCHANGE_START_TYPES,
  groupIntoExchanges,
  renderOrderViolations,
  type Exchange,
  type StoredEvent,
  type ThreadEvent,
} from '../thread-events';

beforeEach(resetSeqCounter);

// ---------------------------------------------------------------------------
// One clock, enforced
//
// The transcript renders strictly by one clock, whatever the source, the actor
// or the request id. `renderOrderViolations` is that rule as code, and this
// file is what keeps it honest.
//
// Three duties. The checker must CATCH a real inversion, so it cannot pass by
// doing nothing. Every boundary type must carry a continuation-handoff
// decision, because the omission of one is what the bug was. And a turn
// interrupted by any boundary that can land mid-turn must still read in order.
//
// See `docs/plans/2026-09-20-the-transcript-reads-by-one-clock.md`.
// ---------------------------------------------------------------------------

function at(second: number): string {
  return `2026-01-01T00:00:${String(second).padStart(2, '0')}Z`;
}

describe('renderOrderViolations catches an inversion', () => {
  it('reports a step rendering above a later boundary', () => {
    const exchanges: Exchange[] = [
      {
        userEvent: { type: 'MessageReceived', text: 'go', created: at(1) } as StoredEvent,
        userSeq: 1,
        steps: [{ seq: 4, event: { type: 'TextStreamed', text: 'late', created: at(4) } as StoredEvent }],
      },
      {
        userEvent: { type: 'ContinuationStarted', created: at(2) } as StoredEvent,
        userSeq: 2,
        steps: [],
      },
    ];
    const violations = renderOrderViolations(exchanges);
    expect(violations).toHaveLength(1);
    expect(violations[0].above.type).toBe('TextStreamed');
    expect(violations[0].below.type).toBe('ContinuationStarted');
  });

  it('accepts the same rows once the boundary comes first', () => {
    const exchanges: Exchange[] = [
      {
        userEvent: { type: 'MessageReceived', text: 'go', created: at(1) } as StoredEvent,
        userSeq: 1,
        steps: [],
      },
      {
        userEvent: { type: 'ContinuationStarted', created: at(2) } as StoredEvent,
        userSeq: 2,
        steps: [{ seq: 4, event: { type: 'TextStreamed', text: 'late', created: at(4) } as StoredEvent }],
      },
    ];
    expect(renderOrderViolations(exchanges)).toEqual([]);
  });
});

describe('every boundary type carries a continuation-handoff decision', () => {
  it('decides exactly the boundary types, and no others', () => {
    // `EventWaitCanceled` is decided by its cause rather than by its type, so
    // it is a boundary without being in the set (`isExchangeStartEvent`).
    const decided = [...BOUNDARY_CONTINUATION_HANDOFF.keys()].sort();
    const boundaries = [...EXCHANGE_START_TYPES, 'EventWaitCanceled'].sort();
    expect(decided).toEqual(boundaries);
  });
});

// ---------------------------------------------------------------------------
// A turn interrupted by any boundary still reads in order
//
// The vocabulary is every boundary a chat turn can meet WHILE RUNNING. Events
// route by request id in that lane. It is therefore the only lane where a row
// can be filed above a card that opened before it.
//
// Coding-agent boundaries are absent on purpose: their events fold by the
// clock, so the continuation lands below whatever opened. The table says the
// same thing, and says it per type.
// ---------------------------------------------------------------------------
const MID_TURN_BOUNDARIES: Array<{ name: string; event: ThreadEvent }> = [
  { name: 'ContinuationStarted', event: { type: 'ContinuationStarted' } as ThreadEvent },
  {
    name: 'UserPromptInjected (a wait waking the thread)',
    event: { type: 'UserPromptInjected', text: 'An event arrived.', mode: 'agent' } as ThreadEvent,
  },
  {
    name: 'UserQuestionAsked',
    event: {
      type: 'UserQuestionAsked',
      tool_use_id: 'tu-1',
      cc_session_id: '',
      question: 'Which one?',
      options: [{ id: 'a', label: 'A' }],
    } as ThreadEvent,
  },
  {
    name: 'CommandPermissionRequested',
    event: {
      type: 'CommandPermissionRequested',
      request_id: 'r-1',
      tool_use_id: 'tu-2',
      tool_name: 'run_bash',
      command: 'ls',
      summary: 'ls',
    } as ThreadEvent,
  },
  {
    name: 'McpPermissionRequested',
    event: {
      type: 'McpPermissionRequested',
      request_id: 'r-2',
      tool_use_id: 'tu-3',
      server_id: 's',
      server_name: 'S',
      tool_name: 't',
      arguments_summary: '{}',
    } as ThreadEvent,
  },
  { name: 'McpConsentRequested', event: { type: 'McpConsentRequested', tool: 't', args: {} } as ThreadEvent },
  {
    name: 'ChildThreadCompleted',
    event: {
      type: 'ChildThreadCompleted',
      child_thread_id: 'c-1',
      status: 'success',
      summary: 'done',
    } as ThreadEvent,
  },
  {
    name: 'MessageReceived (a follow-up still in the queue)',
    event: { type: 'MessageReceived', text: 'also this', channel: 'chat' } as ThreadEvent,
  },
];

describe('a running chat turn keeps its rows in order past any boundary', () => {
  it.each(MID_TURN_BOUNDARIES)('past $name', ({ event }) => {
    const { map, id } = makeThread('thread-1', 'running');
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'go', channel: 'chat', event_id: 'mr-1', created: at(1) },
      { type: 'TextStreamed', text: 'starting', request_event_id: 'mr-1', created: at(2) },
      { ...event, created: at(3) } as ThreadEvent & { created?: string },
      { type: 'TextStreamed', text: 'carrying on', request_event_id: 'mr-1', created: at(4) },
      { type: 'ResponseGenerated', request_event_id: 'mr-1', created: at(5) },
    ] as Array<ThreadEvent & { created?: string; event_id?: string }>);
    expect(renderOrderViolations(getExchanges(map, id))).toEqual([]);
  });
});

// ---------------------------------------------------------------------------
// The three named exceptions, each exercised
//
// An exception nothing reaches is a hole the next reader cannot see. These pin
// the shapes the checker is allowed to forgive, so removing one fails here
// rather than silently widening the rule.
// ---------------------------------------------------------------------------
describe('the named exceptions', () => {
  it('A: a queued message the loop has not picked up yet', () => {
    const { map, id } = makeThread('thread-1', 'running');
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'go', channel: 'chat', event_id: 'mr-1', created: at(1) },
      { type: 'TextStreamed', text: 'starting', request_event_id: 'mr-1', created: at(2) },
      { type: 'MessageReceived', text: 'also this', channel: 'chat', event_id: 'mr-2', created: at(3) },
      // The turn keeps writing while the follow-up waits in the queue.
      { type: 'TextStreamed', text: 'still on the first', request_event_id: 'mr-1', created: at(4) },
      // The loop picks it up here, and everything after reads below the card.
      { type: 'UserPromptInjected', text: 'also this', injected_message_id: 'mr-2', request_event_id: 'mr-1', created: at(5) },
      { type: 'TextStreamed', text: 'on the second now', request_event_id: 'mr-1', created: at(6) },
      { type: 'ResponseGenerated', request_event_id: 'mr-1', created: at(7) },
    ] as Array<ThreadEvent & { created?: string; event_id?: string }>);

    const exchanges = getExchanges(map, id);
    expect(exchanges.map(e => e.userEvent.type)).toEqual(['MessageReceived', 'MessageReceived']);
    // The queued window is real: a row of the first turn sits above the card.
    expect(exchanges[0].steps.map(s => s.seq)).toEqual([2, 4]);
    expect(exchanges[1].steps.map(s => s.seq)).toEqual([5, 6, 7]);
    expect(renderOrderViolations(exchanges)).toEqual([]);
  });

  it('B: a result rejoining the call it answers', () => {
    const { map, id } = makeThread('thread-1', 'running');
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'run it', channel: 'chat', event_id: 'mr-1', created: at(1) },
      { type: 'ToolCalled', name: 'run_bash', args: {}, request_event_id: 'mr-1', event_id: 'call-1', created: at(2) },
      {
        type: 'CommandPermissionRequested',
        request_id: 'r-1', tool_use_id: 'tu-1', tool_name: 'run_bash', command: 'ls', summary: 'ls',
        created: at(3),
      },
      { type: 'CommandPermissionResolved', request_id: 'r-1', allowed: true, created: at(4) },
      // The result completes the call's row, which is above the card.
      { type: 'ToolResult', name: 'run_bash', result: 'ok', tool_called_event_id: 'call-1', request_event_id: 'mr-1', created: at(5) },
      { type: 'ResponseGenerated', request_event_id: 'mr-1', created: at(6) },
    ] as Array<ThreadEvent & { created?: string; event_id?: string }>);

    const exchanges = getExchanges(map, id);
    expect(exchanges[0].steps.map(s => s.event.type)).toEqual(['ToolCalled', 'ToolResult']);
    expect(renderOrderViolations(exchanges)).toEqual([]);
  });

  it('C: a running turn writing past the reader\'s Stop-waiting panel', () => {
    const { map, id } = makeThread('thread-1', 'running');
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'tidy the notes', channel: 'chat', event_id: 'mr-1', created: at(1) },
      { type: 'ToolCalled', name: 'read_file', args: {}, request_event_id: 'mr-1', created: at(2) },
      // The reader stops an unrelated watch while this turn runs.
      { type: 'EventWaitCanceled', wait_id: 'w1', cause: 'user_stop', created: at(3) },
      // Routed by the clock, so it lands in the turn that produced it.
      { type: 'TodoListWritten', items: [{ content: 'tidy', status: 'in_progress' }], created: at(4) },
    ] as Array<ThreadEvent & { created?: string; event_id?: string }>);

    const exchanges = getExchanges(map, id);
    expect(exchanges[1].userEvent.type).toBe('EventWaitCanceled');
    expect(exchanges[1].steps).toHaveLength(0);
    expect(exchanges[0].steps.map(s => s.event.type)).toEqual(['ToolCalled', 'TodoListWritten']);
    expect(renderOrderViolations(exchanges)).toEqual([]);
  });

  it('forgives no row that answers nothing', () => {
    // The same shape as B, with a plain text row in the result's place.
    // Nothing names it, so the checker reports it.
    const events = new Map<number, StoredEvent>([
      [1, { type: 'MessageReceived', text: 'run it', _eventId: 'mr-1', created: at(1) } as StoredEvent],
      [2, { type: 'ToolCalled', name: 'run_bash', args: {}, request_event_id: 'mr-1', _eventId: 'call-1', created: at(2) } as StoredEvent],
      [3, { type: 'CommandPermissionRequested', request_id: 'r-1', tool_use_id: 'tu-1', tool_name: 'run_bash', command: 'ls', summary: 'ls', created: at(3) } as StoredEvent],
      [4, { type: 'ToolResult', name: 'run_bash', result: 'ok', tool_called_event_id: 'call-1', request_event_id: 'mr-1', created: at(4) } as StoredEvent],
    ]);
    const exchanges = groupIntoExchanges(events);
    // Hand the first exchange a row nothing explains, at a later key.
    exchanges[0].steps.push({
      seq: 9,
      event: { type: 'TextStreamed', text: 'out of place', created: at(9) } as StoredEvent,
    });
    expect(renderOrderViolations(exchanges)).toHaveLength(1);
  });
});
