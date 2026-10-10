/**
 * A *form request* renders in the turn that asked for it, so the user can
 * reopen a form they closed or never saw. Its resolution usually lands turns
 * later: the user answers after the agent went quiet, or types a new message
 * instead. The row still has to show how it ended.
 */
import { describe, it, expect } from 'vitest';
import {
  exchangeResponseEvents,
  exchangeResponseTimestamp,
  groupIntoExchanges,
  type StoredEvent,
  type ThreadEvent,
} from '../thread-events';

function ev(seq: number, e: ThreadEvent): readonly [number, StoredEvent] {
  const created = `2026-09-24T12:00:${String(seq).padStart(2, '0')}Z`;
  return [seq, { ...e, created } as StoredEvent] as const;
}

const REQUEST: ThreadEvent = {
  type: 'CredentialRequested',
  request_id: 'req-1',
  payload: JSON.stringify({ service: 'weather', prompt: 'Paste your API key.' }),
};

describe('a form request in the transcript', () => {
  it('renders as a row in the turn that asked, open until answered', () => {
    const exchanges = groupIntoExchanges(new Map([
      ev(1, { type: 'MessageReceived', text: 'connect my weather API' }),
      ev(2, REQUEST),
      ev(3, { type: 'ResponseGenerated', text: 'I sent you a form.' } as ThreadEvent),
    ]));
    expect(exchanges).toHaveLength(1);

    const row = exchangeResponseEvents(exchanges[0]).find(e => e.type === 'form_request');
    expect(row).toMatchObject({ request: { request_id: 'req-1' } });
    expect(row && 'resolution' in row ? row.resolution : undefined).toBeUndefined();
  });

  it('shows the outcome even when the resolution lands in a later turn', () => {
    const exchanges = groupIntoExchanges(new Map([
      ev(1, { type: 'MessageReceived', text: 'connect my weather API' }),
      ev(2, REQUEST),
      ev(3, { type: 'ResponseGenerated', text: 'I sent you a form.' } as ThreadEvent),
      ev(4, { type: 'MessageReceived', text: 'never mind' }),
      ev(5, { type: 'FormRequestResolved', request_id: 'req-1', outcome: 'superseded' }),
    ]));
    expect(exchanges).toHaveLength(2);

    const row = exchangeResponseEvents(exchanges[0]).find(e => e.type === 'form_request');
    expect(row).toMatchObject({ resolution: 'superseded' });
    expect(
      exchanges[1].steps.some(s => s.event.type === 'FormRequestResolved'),
      'the resolution is not a stray step in the turn it happened to land in',
    ).toBe(false);
  });

  it('does not re-date the turn when the answer lands hours later', () => {
    const exchanges = groupIntoExchanges(new Map([
      ev(1, { type: 'MessageReceived', text: 'connect my weather API' }),
      ev(2, REQUEST),
      ev(3, { type: 'ResponseGenerated', text: 'I sent you a form.' } as ThreadEvent),
      ev(59, { type: 'FormRequestResolved', request_id: 'req-1', outcome: 'completed' }),
    ]));
    expect(exchangeResponseTimestamp(exchanges[0])).toBe('2026-09-24T12:00:03Z');
  });

  it('drops a resolution whose request is not on the page', () => {
    const exchanges = groupIntoExchanges(new Map([
      ev(1, { type: 'MessageReceived', text: 'hello' }),
      ev(2, { type: 'FormRequestResolved', request_id: 'off-page', outcome: 'expired' }),
    ]));
    expect(exchanges[0].steps.some(s => s.event.type === 'FormRequestResolved')).toBe(false);
  });
});
