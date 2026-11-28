import { describe, it, expect } from 'vitest';
import { computeExchanges, isWaitingTypedMessage } from './exchange-grouping';
import type { StoredEvent } from './thread-event-types';
import { makeThreadState } from '../__tests__/thread-events-helpers';

const TURN = 'turn-1';
const QUEUED = 'queued-1';
const RESUME = 'cont-1';
const NOTE = '[Engine note] Your previous attempt was interrupted.';

function at(seq: number): string {
  return `2026-09-21T08:57:${String(seq + 40).padStart(2, '0')}Z`;
}

function transcript(upTo: number): Map<number, StoredEvent> {
  const rows: StoredEvent[] = [
    { type: 'MessageReceived', text: 'summarize the tickets', _eventId: TURN, created: at(1) },
    { type: 'MessageReceived', text: 'wtf r u up to?', _eventId: QUEUED, created: at(2) },
    { type: 'ResponseAborted', text: '', cause: 'engine_shutdown', request_event_id: TURN, created: at(3) },
    { type: 'ContinuationStarted', branch: '', _eventId: RESUME, created: at(4) },
    { type: 'PromptInjected', text: NOTE, mode: 'engine', request_event_id: RESUME, created: at(5) },
    { type: 'PromptInjected', text: 'wtf r u up to?', mode: 'human', injected_message_id: QUEUED, request_event_id: RESUME, created: at(6) },
    { type: 'ResponseGenerated', text: 'Reading the tickets now.', request_event_id: RESUME, created: at(7) },
  ] as StoredEvent[];
  return new Map(rows.slice(0, upTo).map((event, i) => [i + 1, event]));
}

function exchangesOf(events: Map<number, StoredEvent>) {
  return computeExchanges(makeThreadState(events));
}

function queuedExchange(upTo: number) {
  return exchangesOf(transcript(upTo)).find(
    ex => ex.userEvent.type === 'MessageReceived' && ex.userEvent._eventId === QUEUED,
  );
}

describe('a queued follow-up recovered after a restart', () => {
  it('reads as Queued while nothing has ingested it', () => {
    const stranded = queuedExchange(5);
    expect(stranded).toBeDefined();
    expect(isWaitingTypedMessage(stranded!)).toBe(true);
  });

  it('stops reading as Queued once the recovery announces it', () => {
    const ingested = queuedExchange(6);
    expect(ingested).toBeDefined();
    expect(isWaitingTypedMessage(ingested!)).toBe(false);
    expect(ingested!.steps.map(s => s.event.type)).toContain('PromptInjected');
  });

  it('gathers the resumed reply under the recovered message', () => {
    const answered = queuedExchange(7);
    expect(answered!.steps.map(s => s.event.type)).toContain('ResponseGenerated');
  });

  it('keeps a retracted message out, even after the resume', () => {
    const rows = new Map(transcript(5));
    rows.set(6, { type: 'QueuedMessageRemoved', removed_message_id: QUEUED, created: at(6) } as StoredEvent);
    const ids = exchangesOf(rows).map(ex => ex.userEvent._eventId);
    expect(ids).not.toContain(QUEUED);
  });
});
