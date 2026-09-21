/**
 * A queued follow-up the engine recovers after a restart stops rendering as
 * "Queued", with no frontend change.
 *
 * The reported bug was engine-side: a follow-up typed mid-turn was stranded
 * when a restart aborted that turn, because nothing read it back out of the
 * event store. The resume now announces it with a `UserPromptInjected` naming
 * it in `injected_message_id`.
 *
 * This pins the client half of that contract. `findAbsorbTarget` folds such a
 * UPI into the message's own exchange, which gives that exchange a step and so
 * ends `isUningestedMessage`. Break the absorb and the bubble stays pinned to
 * the bottom of the transcript even though the engine answered it.
 *
 * Plan: docs/plans/2026-09-21-a-queued-follow-up-survives-the-restart.md
 */
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

/** The reported event sequence, truncated at `upTo` so the before and after of
 *  the recovery can be folded separately. */
function transcript(upTo: number): Map<number, StoredEvent> {
  const rows: StoredEvent[] = [
    { type: 'MessageReceived', text: 'summarize the tickets', _eventId: TURN, created: at(1) },
    { type: 'MessageReceived', text: 'wtf r u up to?', _eventId: QUEUED, created: at(2) },
    { type: 'ResponseAborted', text: '', cause: 'engine_shutdown', request_event_id: TURN, created: at(3) },
    { type: 'ContinuationStarted', branch: '', _eventId: RESUME, created: at(4) },
    { type: 'UserPromptInjected', text: NOTE, mode: 'engine', request_event_id: RESUME, created: at(5) },
    // The recovery: names the stranded message, anchored on the resume.
    { type: 'UserPromptInjected', text: 'wtf r u up to?', mode: 'human', injected_message_id: QUEUED, request_event_id: RESUME, created: at(6) },
    { type: 'ResponseGenerated', text: 'Reading the tickets now.', request_event_id: RESUME, created: at(7) },
  ] as StoredEvent[];
  return new Map(rows.slice(0, upTo).map((event, i) => [i + 1, event]));
}

/** Folded through the entry point the transcript itself renders from, so the
 *  retraction filter is in play the same way it is on screen. */
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
    // Through the abort and the resume boundary: the engine note is not about
    // this message, so the bubble is still waiting. This is the reported state.
    const stranded = queuedExchange(5);
    expect(stranded).toBeDefined();
    expect(isWaitingTypedMessage(stranded!)).toBe(true);
  });

  it('stops reading as Queued once the recovery announces it', () => {
    const ingested = queuedExchange(6);
    expect(ingested).toBeDefined();
    expect(isWaitingTypedMessage(ingested!)).toBe(false);
    expect(ingested!.steps.map(s => s.event.type)).toContain('UserPromptInjected');
  });

  it('gathers the resumed reply under the recovered message', () => {
    // The absorb re-anchors the message to the ingestion point. It redirects
    // the resume's request id onto that exchange too, so the answer renders
    // below the bubble rather than above it.
    const answered = queuedExchange(7);
    expect(answered!.steps.map(s => s.event.type)).toContain('ResponseGenerated');
  });

  it('keeps a retracted message out, even after the resume', () => {
    // A retraction that landed while the engine was down wins: the engine
    // emits no UPI for it, so the exchange is dropped as removed.
    const rows = new Map(transcript(5));
    rows.set(6, { type: 'QueuedMessageRemoved', removed_message_id: QUEUED, created: at(6) } as StoredEvent);
    const ids = exchangesOf(rows).map(ex => ex.userEvent._eventId);
    expect(ids).not.toContain(QUEUED);
  });
});
