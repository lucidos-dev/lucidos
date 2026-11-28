import { describe, expect, it } from 'vitest';
import { ev, said, saidOver } from './call-fixtures';
import { makeThreadState } from './thread-events-helpers';
import { computeExchanges, groupIntoExchanges } from '../thread-events';
import type { Exchange, StoredEvent } from '../thread-events';

const MSG = 'msg-1';

function theCall(reply?: readonly [number, StoredEvent]): Map<number, StoredEvent> {
  const events = new Map<number, StoredEvent>([
    ev(1, { type: 'VoiceSessionStarted', session_id: 'sess-1' }),
    ev(2, { type: 'WorkDelegated', session_id: 'sess-1', reason: 'Check what is waiting.' }),
    ev(3, {
      type: 'MessageReceived',
      text: 'What is it waiting an answer for',
      mode: 'human',
      channel: 'chat',
      voice_session_id: 'sess-1',
      _eventId: MSG,
    }),
    ev(20, { type: 'MemoryRecalled', results: 25, queries: ['pending tasks'], request_event_id: MSG }),
    ev(30, { type: 'ToolCalled', name: 'list_threads', args: {}, _eventId: 'tc-1', request_event_id: MSG }),
    ev(40, { type: 'ToolResult', name: 'list_threads', result: 'ok', request_event_id: MSG }),
  ]);
  if (reply) events.set(reply[0], reply[1]);
  return events;
}

function theTurn(exchanges: Exchange[]): Exchange {
  const turn = exchanges.find(e => e.userEvent._eventId === MSG);
  expect(turn).toBeDefined();
  return turn as Exchange;
}

function stepTypes(exchange: Exchange): string[] {
  return exchange.steps.map(s => s.event.type);
}

describe("the engine's own row", () => {
  it('reads above the work it promised', () => {
    const turn = theTurn(groupIntoExchanges(theCall(said(10, "I'm on it, give me a sec."))));
    expect(stepTypes(turn)).toEqual([
      'SpokenReplyGenerated',
      'MemoryRecalled',
      'ToolCalled',
      'ToolResult',
    ]);
  });

  it('reads between the steps it fell between', () => {
    const turn = theTurn(groupIntoExchanges(theCall(said(25, 'Still working on it.'))));
    expect(stepTypes(turn)).toEqual([
      'MemoryRecalled',
      'SpokenReplyGenerated',
      'ToolCalled',
      'ToolResult',
    ]);
  });

  it('reads below work that finished before it', () => {
    const turn = theTurn(groupIntoExchanges(theCall(said(50, 'Three threads are waiting.'))));
    expect(stepTypes(turn)).toEqual([
      'MemoryRecalled',
      'ToolCalled',
      'ToolResult',
      'SpokenReplyGenerated',
    ]);
  });

  it('leaves a session mark at the end', () => {
    const events = theCall(said(10, "I'm on it, give me a sec."));
    const [seq, ended] = ev(50, {
      type: 'VoiceSessionEnded',
      session_id: 'sess-1',
      reason: 'hangup',
      duration_secs: 12,
    });
    events.set(seq, ended);
    expect(stepTypes(theTurn(groupIntoExchanges(events)))).toEqual([
      'SpokenReplyGenerated',
      'MemoryRecalled',
      'ToolCalled',
      'ToolResult',
      'VoiceSessionEnded',
    ]);
  });
});

describe('the walk that places it', () => {
  it('keeps two replies of one call in the order they were said', () => {
    const events = theCall(said(10, "I'm on it,"));
    const [seq, second] = said(25, 'Still working on it.');
    events.set(seq, second);
    expect(stepTypes(theTurn(groupIntoExchanges(events)))).toEqual([
      'SpokenReplyGenerated',
      'MemoryRecalled',
      'SpokenReplyGenerated',
      'ToolCalled',
      'ToolResult',
    ]);
  });

  it('stays below a run of streamed text it came after', () => {
    const events = new Map<number, StoredEvent>([
      ev(1, {
        type: 'MessageReceived',
        text: 'what is waiting',
        mode: 'human',
        channel: 'chat',
        voice_session_id: 'sess-1',
        _eventId: MSG,
      }),
      ev(2, { type: 'MemoryRecalled', results: 3, queries: ['waiting'], request_event_id: MSG }),
      ev(30, { type: 'TextStreamed', text: 'Three threads are', request_event_id: MSG }),
      ev(40, { type: 'TextStreamed', text: ' waiting on', request_event_id: MSG }),
      ev(50, { type: 'TextStreamed', text: ' you.', request_event_id: MSG }),
      ev(55, { type: 'ToolCalled', name: 'send_notification', args: {}, request_event_id: MSG }),
    ]);
    const [seq, note] = said(52, 'That is everything.');
    events.set(seq, note);
    expect(stepTypes(theTurn(groupIntoExchanges(events)))).toEqual([
      'MemoryRecalled',
      'TextStreamed',
      'TextStreamed',
      'TextStreamed',
      'SpokenReplyGenerated',
      'ToolCalled',
    ]);
  });

  it('never lands inside a run of streamed text', () => {
    const events = new Map<number, StoredEvent>([
      ev(1, {
        type: 'MessageReceived',
        text: 'what is waiting',
        mode: 'human',
        channel: 'chat',
        voice_session_id: 'sess-1',
        _eventId: MSG,
      }),
      ev(2, { type: 'MemoryRecalled', results: 3, queries: ['waiting'], request_event_id: MSG }),
      ev(30, { type: 'TextStreamed', text: 'Three threads are', request_event_id: MSG }),
      ev(40, { type: 'TextStreamed', text: ' waiting on', request_event_id: MSG }),
      ev(50, { type: 'TextStreamed', text: ' you.', request_event_id: MSG }),
      ev(55, { type: 'ToolCalled', name: 'send_notification', args: {}, request_event_id: MSG }),
    ]);
    const [seq, note] = said(45, 'Still working on it.');
    events.set(seq, note);
    expect(stepTypes(theTurn(groupIntoExchanges(events)))).toEqual([
      'MemoryRecalled',
      'SpokenReplyGenerated',
      'TextStreamed',
      'TextStreamed',
      'TextStreamed',
      'ToolCalled',
    ]);
  });
});

describe('a reply said over the work', () => {
  it('reads above the step it began before', () => {
    const turn = theTurn(groupIntoExchanges(theCall(saidOver(25, 10, "Good idea. I'm on it."))));
    expect(stepTypes(turn)).toEqual([
      'SpokenReplyGenerated',
      'MemoryRecalled',
      'ToolCalled',
      'ToolResult',
    ]);
  });

  it('reads below a step it began after', () => {
    const turn = theTurn(groupIntoExchanges(theCall(saidOver(35, 2, 'Three threads are waiting.'))));
    expect(stepTypes(turn)).toEqual([
      'MemoryRecalled',
      'ToolCalled',
      'SpokenReplyGenerated',
      'ToolResult',
    ]);
  });

  it('stays one bubble when a step lands mid-sentence', () => {
    const events = theCall(saidOver(25, 10, 'Good idea.'));
    const [seq, tail] = saidOver(28, 2, "I'm on it.");
    events.set(seq, tail);
    const turn = theTurn(groupIntoExchanges(events));
    expect(stepTypes(turn)).toEqual([
      'SpokenReplyGenerated',
      'MemoryRecalled',
      'ToolCalled',
      'ToolResult',
    ]);
    const merged = turn.steps[0].event as { text: string; spoken_secs_before?: number };
    expect(merged.text).toBe("Good idea. I'm on it.");
    expect(merged.spoken_secs_before).toBe(13);
  });

  it('is two bubbles when the step landed in the silence between them', () => {
    const events = theCall(saidOver(18, 3, 'Let me check.'));
    const [seq, second] = saidOver(22, 1, 'Still going.');
    events.set(seq, second);
    expect(stepTypes(theTurn(groupIntoExchanges(events)))).toEqual([
      'SpokenReplyGenerated',
      'MemoryRecalled',
      'SpokenReplyGenerated',
      'ToolCalled',
      'ToolResult',
    ]);
  });
});

describe('the live row', () => {
  function theLiveCall(): Exchange[] {
    const thread = makeThreadState(theCall());
    thread.liveReply = {
      eventId: 'live-reply:thread-1:1',
      created: '2026-08-31T07:15:10Z',
      text: "I'm on it,",
    };
    return computeExchanges(thread);
  }

  it("reads above the work it promised, exactly as the engine's row will", () => {
    expect(stepTypes(theTurn(theLiveCall()))).toEqual([
      'SpokenReplyGenerated',
      'MemoryRecalled',
      'ToolCalled',
      'ToolResult',
    ]);
  });

  it('publishes its words for the memo to compare', () => {
    expect(theTurn(theLiveCall()).liveReplyText).toBe("I'm on it,");
  });

  it('lands exactly where the engine row for it will', () => {
    const landed = groupIntoExchanges(theCall(saidOver(25, 15, "I'm on it,")));
    expect(stepTypes(theTurn(landed))).toEqual(stepTypes(theTurn(theLiveCall())));
  });

  it('never reads above the reply it followed', () => {
    const thread = makeThreadState(theCall(saidOver(25, 10, "I'm on it,")));
    thread.liveReply = {
      eventId: 'live-reply:thread-1:2',
      created: '2026-08-31T07:15:20Z',
      text: 'Still going',
    };
    expect(stepTypes(theTurn(computeExchanges(thread)))).toEqual([
      'SpokenReplyGenerated',
      'MemoryRecalled',
      'SpokenReplyGenerated',
      'ToolCalled',
      'ToolResult',
    ]);
  });
});
