/** A spoken row reads where it happened, by the clock and nothing else.
 *
 *  The reported call drew `I'm on it, give me a sec.` under fifteen seconds of
 *  the work it promised.
 *
 *  The engine writes each turn down as that turn ends, so `created` IS when
 *  the words stopped (ADR 0201). This file pins what the transcript makes of
 *  that, on both ends the reader meets: the live row the client draws from the
 *  deltas, and the engine's own row that replaces it. They must land in the
 *  same place or the swap moves the bubble.
 *
 *  Plan: `docs/plans/2026-09-16-the-clock-is-the-only-order.md`.
 */
import { describe, expect, it } from 'vitest';
import { ev, said } from './call-fixtures';
import { makeThreadState } from './thread-events-helpers';
import { computeExchanges, groupIntoExchanges } from '../thread-events';
import type { Exchange, StoredEvent } from '../thread-events';

const MSG = 'msg-1';

/** A delegated question, the turn it started, and room to speak inside it.
 *
 *  `ev` stamps second `n` onto seq `n`, so the doer's three steps land at :20,
 *  :30 and :40. The gaps are where a reply goes. */
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

/** The turn the doer ran, which is where a call row is filed. */
function theTurn(exchanges: Exchange[]): Exchange {
  const turn = exchanges.find(e => e.userEvent._eventId === MSG);
  expect(turn).toBeDefined();
  return turn as Exchange;
}

function stepTypes(exchange: Exchange): string[] {
  return exchange.steps.map(s => s.event.type);
}

describe("the engine's own row", () => {
  /** Written at :10, which is ten seconds before the turn's first step. So it
   *  reads above all three of them. */
  it('reads above the work it promised', () => {
    const turn = theTurn(groupIntoExchanges(theCall(said(10, "I'm on it, give me a sec."))));
    expect(stepTypes(turn)).toEqual([
      'SpokenReplyGenerated',
      'MemoryRecalled',
      'ToolCalled',
      'ToolResult',
    ]);
  });

  /** A progress note said mid-turn reads mid-turn. The row is not hoisted to
   *  the top of the steps: it goes where the words were. */
  it('reads between the steps it fell between', () => {
    const turn = theTurn(groupIntoExchanges(theCall(said(25, 'Still working on it.'))));
    expect(stepTypes(turn)).toEqual([
      'MemoryRecalled',
      'SpokenReplyGenerated',
      'ToolCalled',
      'ToolResult',
    ]);
  });

  /** Words the caller heard after every step read after every step. */
  it('reads below work that finished before it', () => {
    const turn = theTurn(groupIntoExchanges(theCall(said(50, 'Three threads are waiting.'))));
    expect(stepTypes(turn)).toEqual([
      'MemoryRecalled',
      'ToolCalled',
      'ToolResult',
      'SpokenReplyGenerated',
    ]);
  });

  /** A session mark is not speech, and the end is where it belongs. */
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
  /** Every spoken line reads back in the order it was said, however many of
   *  them one call holds. */
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

  /** `TextStreamed` persists one row per delta, and the renderer joins only
   *  ADJACENT ones. A row dropped mid-run therefore cuts one answer into two
   *  markdown documents, and the collapse keeps the second alone. So the row
   *  goes to the run's start, which is also where the reader expects a note
   *  said while the answer was being written. */
  /** A row that merely FOLLOWS a finished run is not inside it.
   *
   *  Backing out on the left alone lifted such a row above the answer it came
   *  after. The live row that drew it sat below, so the bubble jumped three
   *  rows as the engine's row landed. */
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
    // Said between the second and third delta. Placed by time alone it would
    // sit there and split the answer.
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

describe('the live row', () => {
  /** The bridge draws it the moment the talker's first word arrives, which is
   *  before the doer's steps. It must file there too, or the reader watches
   *  the bubble jump when the engine's row lands. */
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

  /** The memo cannot see a step whose text moves under a stable seq, so the
   *  exchange carries the words as a field of its own. Without it the bubble
   *  froze on whatever prefix the first render caught. */
  it('publishes its words for the memo to compare', () => {
    expect(theTurn(theLiveCall()).liveReplyText).toBe("I'm on it,");
  });
});
