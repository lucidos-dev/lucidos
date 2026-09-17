/** A delegated call, in the shape the engine writes.
 *
 *  The caller's words are a `SpokenMessageReceived` and the talker's
 *  `WorkDelegated` starts the turn (ADR 0201). No `MessageReceived` is written,
 *  so every reader that told a delegated utterance apart by that type is
 *  reading a row the engine no longer emits.
 *
 *  That is what this file pins. The client's other call fixtures still build
 *  the OLD shape on purpose: those rows are in the store and must keep
 *  rendering. What none of them covered is the new one.
 *
 *  Plan: `docs/plans/2026-09-16-the-clock-is-the-only-order.md`.
 */
import { describe, expect, it } from 'vitest';
import { ev, heard, put, said } from './call-fixtures';
import { exchangeStatus, groupIntoExchanges } from '../thread-events';
import type { Exchange, StoredEvent } from '../thread-events';

/** The `WorkDelegated` row, which is the turn's anchor. */
const DELEGATION = 'wd-1';

function aDelegatedCall(): Map<number, StoredEvent> {
  return new Map([
    ev(1, { type: 'VoiceSessionStarted', session_id: 'sess-1' }),
    said(2, 'Hi there. How can I help?'),
    heard(3, "What's going on in the codebase today?"),
    ev(4, {
      type: 'WorkDelegated',
      session_id: 'sess-1',
      reason: 'Check the workspace.',
      _eventId: DELEGATION,
    }),
  ]);
}

/** The card the caller's words opened, which is where the turn shows. */
function theCard(exchanges: Exchange[]): Exchange {
  const card = exchanges.find(e => e.userEvent.type === 'SpokenMessageReceived');
  expect(card).toBeDefined();
  return card as Exchange;
}

function stepTypes(exchange: Exchange): string[] {
  return exchange.steps.map(s => s.event.type);
}

describe('a delegated call', () => {
  it('draws the words and the delegation as one card', () => {
    const exchanges = groupIntoExchanges(aDelegatedCall());
    expect(exchanges.map(e => e.userEvent.type)).toEqual([
      'SpokenReplyGenerated',
      'SpokenMessageReceived',
    ]);
    expect(stepTypes(theCard(exchanges))).toEqual(['WorkDelegated']);
  });

  it('marks that card as holding the turn', () => {
    expect(theCard(groupIntoExchanges(aDelegatedCall())).tookTheTurn).toBe(true);
  });

  /** The turn's events carry the `WorkDelegated` id as their anchor, and no
   *  exchange wears that id. Without the fold's redirect they would fall
   *  through to whatever happens to be open. */
  it('files the doer work under it, by the anchor the turn carries', () => {
    const events = aDelegatedCall();
    put(events, 20, { type: 'MemoryRecalled', results: 3, queries: ['codebase'], request_event_id: DELEGATION });
    put(events, 30, { type: 'ToolCalled', name: 'run_bash', args: {}, request_event_id: DELEGATION });
    put(events, 40, { type: 'ResponseGenerated', text: 'Sixty commits.', request_event_id: DELEGATION });
    expect(stepTypes(theCard(groupIntoExchanges(events)))).toEqual([
      'WorkDelegated',
      'MemoryRecalled',
      'ToolCalled',
      'ResponseGenerated',
    ]);
  });

  /** The loop acknowledges a message it picked up mid-turn with a
   *  `UserPromptInjected` naming the anchor. Unabsorbed it opens a boundary of
   *  its own, and the reader meets their own sentence twice. */
  it('absorbs the acknowledgement rather than drawing the words twice', () => {
    const events = aDelegatedCall();
    put(events, 20, {
      type: 'UserPromptInjected',
      text: "What's going on in the codebase today?",
      mode: 'human',
      injected_message_id: DELEGATION,
    });
    const exchanges = groupIntoExchanges(events);
    expect(exchanges.map(e => e.userEvent.type)).toEqual([
      'SpokenReplyGenerated',
      'SpokenMessageReceived',
    ]);
    expect(stepTypes(theCard(exchanges))).toEqual(['WorkDelegated', 'UserPromptInjected']);
  });

  /** The talker stalls while the doer works, routinely before the doer has
   *  woken. Read as the answer, the card settles Done seconds in and flips
   *  back to Working when the first step lands. */
  it('is not settled by the talker stalling for the doer', () => {
    const events = aDelegatedCall();
    put(events, 20, {
      type: 'SpokenReplyGenerated',
      session_id: 'sess-1',
      text: 'Let me check that for you.',
      interrupted: false,
    });
    const card = theCard(groupIntoExchanges(events));
    expect(exchangeStatus(card, '', true, false, false, true)).toBe('streaming');
  });

  /** The talker stalls before it asks, which is the ordinary Live shape.
   *
   *  That stall is its own turn, so its row lands in the caller's card BEFORE
   *  the delegation does (ADR 0201). A guard asking for an empty card missed
   *  every call shaped like this, and the card then settled Done while the
   *  doer worked.
   */
  it('holds the turn when the talker stalled before asking', () => {
    const events = aDelegatedCall();
    // Said at :05, between the utterance and the delegation at :04... so the
    // delegation is re-stamped later to keep the real order.
    events.delete(4);
    put(events, 5, {
      type: 'SpokenReplyGenerated',
      session_id: 'sess-1',
      text: "I'm on it.",
      interrupted: false,
    });
    put(events, 6, {
      type: 'WorkDelegated',
      session_id: 'sess-1',
      reason: 'Check the workspace.',
      _eventId: DELEGATION,
    });
    put(events, 20, { type: 'ToolCalled', name: 'run_bash', args: {}, request_event_id: DELEGATION });
    const card = theCard(groupIntoExchanges(events));
    expect(card.tookTheTurn).toBe(true);
    // And the doer's work still files under it, by the anchor it carries.
    expect(stepTypes(card)).toEqual([
      'SpokenReplyGenerated',
      'WorkDelegated',
      'ToolCalled',
    ]);
  });

  /** A row order from before ADR 0201, which is still in the store.
   *
   *  There the delegation came FIRST and the `MessageReceived` after it was
   *  the turn's starter. Marking whatever the delegation happened to follow
   *  gave a greeting a turn it never held, and a finished old call then read
   *  Aborted: the regression `a-finished-call-is-not-aborted` exists for. */
  it('leaves a legacy row order alone', () => {
    const exchanges = groupIntoExchanges(new Map([
      ev(1, { type: 'VoiceSessionStarted', session_id: 'sess-1' }),
      said(2, 'Hi there. How can I help?'),
      ev(3, { type: 'WorkDelegated', session_id: 'sess-1', reason: 'Check the release.' }),
      ev(4, {
        type: 'MessageReceived',
        text: 'How long will the release take?',
        mode: 'human',
        channel: 'chat',
        voice_session_id: 'sess-1',
        _eventId: 'legacy-msg',
      }),
    ]));
    expect(exchanges.map(e => e.tookTheTurn)).toEqual([undefined, undefined]);
  });

  /** The same, with an utterance the talker had already handled above it. The
   *  delegation lands on that card and must not mark it either. */
  it('leaves a legacy order alone under an earlier utterance', () => {
    const exchanges = groupIntoExchanges(new Map([
      ev(1, { type: 'VoiceSessionStarted', session_id: 'sess-1' }),
      heard(2, 'hei'),
      ev(3, { type: 'WorkDelegated', session_id: 'sess-1', reason: 'Check the release.' }),
      ev(4, {
        type: 'MessageReceived',
        text: 'How long will the release take?',
        mode: 'human',
        channel: 'chat',
        voice_session_id: 'sess-1',
        _eventId: 'legacy-msg',
      }),
    ]));
    expect(exchanges.map(e => e.tookTheTurn)).toEqual([undefined, undefined]);
  });

  /** A hangup settles the CALL, never the work. The doer's answer outlives the
   *  line, so a turn that produced nothing is still a turn that produced
   *  nothing. */
  it('is not settled by the caller ringing off either', () => {
    const events = aDelegatedCall();
    put(events, 20, {
      type: 'SpokenReplyGenerated',
      session_id: 'sess-1',
      text: 'Let me check that for you.',
      interrupted: false,
    });
    put(events, 30, { type: 'VoiceSessionEnded', session_id: 'sess-1', reason: 'hangup', duration_secs: 30 });
    const card = theCard(groupIntoExchanges(events));
    expect(exchangeStatus(card, '', true, false, false, true)).not.toBe('done');
  });
});
