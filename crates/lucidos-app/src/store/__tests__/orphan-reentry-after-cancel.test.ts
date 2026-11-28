import { describe, it, expect } from 'vitest';
import {
  groupIntoExchanges,
  exchangeStatus,
  type StoredEvent,
  type ThreadEvent,
} from '../thread-events';
import { isActive } from '../exchange-status';
import { makeExchange, step } from './fixtures';

const MSG_A = 'evt-a';
const MSG_B = 'evt-b';

function ev(
  seq: number,
  e: ThreadEvent,
  eventId?: string,
): readonly [number, StoredEvent] {
  const created = `2026-08-04T19:42:${String(seq).padStart(2, '0')}Z`;
  return [seq, { ...e, created, ...(eventId ? { _eventId: eventId } : {}) } as StoredEvent];
}

function thread(...entries: Array<readonly [number, StoredEvent]>) {
  return new Map(entries);
}

function cancelThenOrphanReentry() {
  return thread(
    ev(1, { type: 'MessageReceived', text: 'summarize the report' }, MSG_A),
    ev(2, { type: 'MessageReceived', text: 'actually, just the totals' }, MSG_B),
    ev(3, { type: 'ResponseCanceled', cause: 'user_stop', request_event_id: MSG_A } as ThreadEvent),
    ev(4, {
      type: 'PromptInjected',
      text: 'actually, just the totals',
      mode: 'human',
      injected_message_id: MSG_B,
      request_event_id: MSG_B,
    } as ThreadEvent),
  );
}

describe('orphan re-entry after a user Stop', () => {
  it('re-anchors the follow-up below the cancel boundary', () => {
    const exchanges = groupIntoExchanges(cancelThenOrphanReentry());

    expect(exchanges).toHaveLength(3);
    expect(exchanges[0].userEvent._eventId).toBe(MSG_A);
    expect(exchanges[1].userEvent.type).toBe('ResponseCanceled');
    expect(exchanges[2].userEvent._eventId).toBe(MSG_B);
  });

  it('absorbs the announcement instead of rendering a second panel for the follow-up', () => {
    const exchanges = groupIntoExchanges(cancelThenOrphanReentry());

    expect(exchanges.map(e => e.userEvent.type)).toEqual([
      'MessageReceived',
      'ResponseCanceled',
      'MessageReceived',
    ]);
    expect(exchanges[2].steps.map(s => s.event.type)).toEqual(['PromptInjected']);
  });

  it('reads the cancelled turn as canceled and the follow-up as live', () => {
    const exchanges = groupIntoExchanges(cancelThenOrphanReentry());
    const last = exchanges.length - 1;

    expect(exchangeStatus(exchanges[0], '', false)).toBe('canceled');

    const followup = exchangeStatus(exchanges[last], '', true, false, false, false);
    expect(isActive(followup)).toBe(true);
    expect(followup).not.toBe('done');
  });

  it('still reads as live before the re-submitted turn emits its first step', () => {
    const exchanges = groupIntoExchanges(cancelThenOrphanReentry());
    const followup = exchanges[exchanges.length - 1];

    expect(followup.steps).toHaveLength(1);
    expect(isActive(exchangeStatus(followup, '', true, false, false, false))).toBe(true);
  });
});

describe('absorbed-injection placeholder, the mid-flight shape it exists for', () => {
  it('still reads done when the announcement anchors on a prior turn', () => {
    const events = thread(
      ev(1, { type: 'MessageReceived', text: 'first' }, MSG_A),
      ev(2, { type: 'MessageReceived', text: 'second' }, MSG_B),
      ev(3, {
        type: 'PromptInjected',
        text: 'second',
        mode: 'human',
        injected_message_id: MSG_B,
        request_event_id: MSG_A,
      } as ThreadEvent),
    );
    const exchanges = groupIntoExchanges(events);
    const followup = exchanges[exchanges.length - 1];

    expect(followup.userEvent._eventId).toBe(MSG_B);
    expect(exchangeStatus(followup, '', true, false, false, false)).toBe('done');
  });

  it('still reads done for a legacy announcement with no request id', () => {
    const events = thread(
      ev(1, { type: 'MessageReceived', text: 'first' }, MSG_A),
      ev(2, { type: 'MessageReceived', text: 'second' }, MSG_B),
      ev(3, {
        type: 'PromptInjected',
        text: 'second',
        mode: 'human',
        injected_message_id: MSG_B,
      } as ThreadEvent),
    );
    const exchanges = groupIntoExchanges(events);
    const followup = exchanges[exchanges.length - 1];

    expect(exchangeStatus(followup, '', true, false, false, false)).toBe('done');
  });

  it('still reads done when neither the message nor the announcement has an id', () => {
    const followup = makeExchange({ type: 'MessageReceived', text: 'second' } as StoredEvent, [
      step(1, {
        type: 'PromptInjected',
        text: 'second',
        injected_message_id: 'legacy-followup',
      } as StoredEvent),
    ]);

    expect(exchangeStatus(followup, '', true, false, false, true)).toBe('done');
  });
});
