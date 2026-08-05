/** The shape a user Stop leaves behind when a follow-up was already in flight.
 *
 *  Real thread, 2026-08-04: message A starts a turn, the user clicks Stop,
 *  message B lands while the turn is still unwinding (so it is injected into
 *  the running turn rather than queued behind a fresh one), then A's
 *  `ResponseCanceled` finally arrives. B is recovered as an orphan and
 *  re-submitted as a turn of its own, anchored on the `MessageReceived` that
 *  already exists for it. The engine announces that ingestion with a
 *  `UserPromptInjected` naming B (`announce_orphan_batch` in `api/chat.rs`).
 *
 *  Two things must come out of the fold: B is re-anchored BELOW the cancel
 *  boundary (the agent engaged with it after the cancel, not before), and B
 *  reads as the live turn rather than "Done ✓". Before the fix the cancel panel
 *  sat last, B sat above it reading "Done", and the thread looked finished for
 *  the whole minute it took to answer.
 *
 *  See `docs/plans/2026-08-04-chat-stop-honored-during-turn-setup.md`.
 */
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

/** Message A, message B, A's cancel, then the orphan re-entry announcing B. */
function cancelThenOrphanReentry() {
  return thread(
    ev(1, { type: 'MessageReceived', text: 'summarize the report' }, MSG_A),
    ev(2, { type: 'MessageReceived', text: 'actually, just the totals' }, MSG_B),
    ev(3, { type: 'ResponseCanceled', cause: 'user_stop', request_event_id: MSG_A } as ThreadEvent),
    ev(4, {
      type: 'UserPromptInjected',
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

    // Three exchanges, not four: the UPI is a step of B, not a boundary.
    expect(exchanges.map(e => e.userEvent.type)).toEqual([
      'MessageReceived',
      'ResponseCanceled',
      'MessageReceived',
    ]);
    expect(exchanges[2].steps.map(s => s.event.type)).toEqual(['UserPromptInjected']);
  });

  it('reads the cancelled turn as canceled and the follow-up as live', () => {
    const exchanges = groupIntoExchanges(cancelThenOrphanReentry());
    const last = exchanges.length - 1;

    expect(exchangeStatus(exchanges[0], '', false)).toBe('canceled');

    // The thread is running the re-submitted turn, so `threadIdle` is false.
    const followup = exchangeStatus(exchanges[last], '', true, false, false, false);
    expect(isActive(followup)).toBe(true);
    expect(followup).not.toBe('done');
  });

  it('still reads as live before the re-submitted turn emits its first step', () => {
    // The gap the user actually saw: ~31s between the cancel and the new turn's
    // first event. The follow-up's only step is the announcement itself.
    const exchanges = groupIntoExchanges(cancelThenOrphanReentry());
    const followup = exchanges[exchanges.length - 1];

    expect(followup.steps).toHaveLength(1);
    expect(isActive(exchangeStatus(followup, '', true, false, false, false))).toBe(true);
  });
});

describe('absorbed-UPI placeholder, the mid-flight shape it exists for', () => {
  /** The original case: the loop ingests a queued follow-up MID-TURN, so every
   *  event after it still carries the FIRST message's request id and the answer
   *  renders in that earlier exchange. The follow-up's panel is a placeholder
   *  and must read "done", never a spinner. */
  it('still reads done when the announcement anchors on a prior turn', () => {
    const events = thread(
      ev(1, { type: 'MessageReceived', text: 'first' }, MSG_A),
      ev(2, { type: 'MessageReceived', text: 'second' }, MSG_B),
      ev(3, {
        type: 'UserPromptInjected',
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

  /** Legacy rows predate `request_event_id` on the payload, so the absent id
   *  can never name this exchange and the classification is unchanged. */
  it('still reads done for a legacy announcement with no request id', () => {
    const events = thread(
      ev(1, { type: 'MessageReceived', text: 'first' }, MSG_A),
      ev(2, { type: 'MessageReceived', text: 'second' }, MSG_B),
      ev(3, {
        type: 'UserPromptInjected',
        text: 'second',
        mode: 'human',
        injected_message_id: MSG_B,
      } as ThreadEvent),
    );
    const exchanges = groupIntoExchanges(events);
    const followup = exchanges[exchanges.length - 1];

    expect(exchangeStatus(followup, '', true, false, false, false)).toBe('done');
  });

  /** The oldest rows carry NEITHER id. Two `undefined`s must not compare equal
   *  into "this announcement names my own message": that would revoke the
   *  placeholder from the exact shape it was written for and drop it through to
   *  the stale detector, surfacing "Aborted ⚠" on an exchange that was answered
   *  in the turn above it. */
  it('still reads done when neither the message nor the announcement has an id', () => {
    // Built directly rather than folded: a legacy row has no `_eventId` for the
    // absorb to match on, so the fold would make the announcement its own
    // boundary. What is under test is the status verdict, not the grouping.
    const followup = makeExchange({ type: 'MessageReceived', text: 'second' } as StoredEvent, [
      step(1, {
        type: 'UserPromptInjected',
        text: 'second',
        injected_message_id: 'legacy-followup',
      } as StoredEvent),
    ]);

    // `threadIdle` true: the answer landed in the prior exchange and the thread
    // settled, which is when the stale detector would otherwise fire.
    expect(exchangeStatus(followup, '', true, false, false, true)).toBe('done');
  });
});
