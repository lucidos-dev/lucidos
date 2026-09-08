/**
 * `queuedMessagesFromExchanges` derives the queued (un-injected) chat follow-ups
 * a user Stop should return to compose — from the SAME `queuedFollowupRun` the UI
 * renders "Queued" bubbles from, so Stop clears exactly what the user saw queued.
 */
import { describe, it, expect } from 'vitest';
import { queuedMessagesFromExchanges } from './exchange-render';
import type { Exchange } from './exchange';
import type { SequencedEvent } from './thread-event-types';

const TS = '2026-07-19T00:00:00Z';

function streamStep(): SequencedEvent {
  return { seq: 2, event: { type: 'TextStreamed', text: 'working', created: TS } as SequencedEvent['event'] };
}

/** A stepless MessageReceived exchange = a queued (uningested) follow-up. */
function queued(text: string, id?: string): Exchange {
  return {
    userEvent: { type: 'MessageReceived', text, created: TS, ...(id ? { _eventId: id } : {}) } as Exchange['userEvent'],
    userSeq: 1,
    steps: [],
  };
}

/** A non-terminal streaming turn — the active exchange follow-ups queue behind. */
function activeStreaming(): Exchange {
  return {
    userEvent: { type: 'MessageReceived', text: 'active', created: TS, _eventId: 'mr-active' } as Exchange['userEvent'],
    userSeq: 1,
    steps: [streamStep()],
  };
}

describe('queuedMessagesFromExchanges', () => {
  it('returns queued follow-ups in FIFO order with id + text', () => {
    const list = [activeStreaming(), queued('first queued', 'q1'), queued('second queued', 'q2')];
    expect(queuedMessagesFromExchanges(list, true, false)).toEqual([
      { id: 'q1', text: 'first queued' },
      { id: 'q2', text: 'second queued' },
    ]);
  });

  it('returns nothing when the thread is not busy', () => {
    const list = [activeStreaming(), queued('q', 'q1')];
    expect(queuedMessagesFromExchanges(list, false, false)).toEqual([]);
  });

  it('returns nothing for CC/Codex threads (follow-ups go to stdin, never queued)', () => {
    const list = [activeStreaming(), queued('q', 'q1')];
    expect(queuedMessagesFromExchanges(list, true, true)).toEqual([]);
  });

  it('skips a queued exchange with no _eventId (cannot be retracted by id)', () => {
    const list = [activeStreaming(), queued('no id'), queued('has id', 'q2')];
    expect(queuedMessagesFromExchanges(list, true, false)).toEqual([{ id: 'q2', text: 'has id' }]);
  });

  it('keeps a queued follow-up queued when an async row merely landed in it', () => {
    // A queued message is `current` for as long as the running turn lasts, and
    // `BackgroundBashCompleted` carries no `request_event_id`, so the fold
    // files it there. Read as a step, the message looks ingested: it steals the
    // running turn's badge and Stop no longer returns its text to compose,
    // while the loop still runs it after the cancel.
    const withAsyncRow = queued('follow up', 'q1');
    withAsyncRow.steps = [{
      seq: 3,
      event: { type: 'BackgroundBashCompleted', created: TS } as SequencedEvent['event'],
    }];
    const list = [activeStreaming(), withAsyncRow];
    expect(queuedMessagesFromExchanges(list, true, false)).toEqual([
      { id: 'q1', text: 'follow up' },
    ]);
  });

  it('treats the first of several stepless messages (no active turn) as active, the rest queued', () => {
    // Mirrors "a freshly-sent first message is active (Requesting), not queued":
    // with no non-uningested turn, the earliest candidate owns the active slot.
    const list = [queued('first', 'q1'), queued('second', 'q2')];
    expect(queuedMessagesFromExchanges(list, true, false)).toEqual([{ id: 'q2', text: 'second' }]);
  });
});
