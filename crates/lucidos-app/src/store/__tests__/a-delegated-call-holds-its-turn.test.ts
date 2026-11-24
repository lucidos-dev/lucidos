import { describe, expect, it } from 'vitest';
import { ev, heard, put, said } from './call-fixtures';
import { exchangeStatus, groupIntoExchanges } from '../thread-events';
import type { Exchange, StoredEvent } from '../thread-events';

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

  it('absorbs the acknowledgement rather than drawing the words twice', () => {
    const events = aDelegatedCall();
    put(events, 20, {
      type: 'PromptInjected',
      text: "What's going on in the codebase today?",
      mode: 'human',
      injected_message_id: DELEGATION,
    });
    const exchanges = groupIntoExchanges(events);
    expect(exchanges.map(e => e.userEvent.type)).toEqual([
      'SpokenReplyGenerated',
      'SpokenMessageReceived',
    ]);
    expect(stepTypes(theCard(exchanges))).toEqual(['WorkDelegated', 'PromptInjected']);
  });

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

  it('holds the turn when the talker stalled before asking', () => {
    const events = aDelegatedCall();
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
    expect(stepTypes(card)).toEqual([
      'SpokenReplyGenerated',
      'WorkDelegated',
      'ToolCalled',
    ]);
  });

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
    expect(exchangeStatus(card, '', true, false, false,  false)).not.toBe('done');
  });
});
