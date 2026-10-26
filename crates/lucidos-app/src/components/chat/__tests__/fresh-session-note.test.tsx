import { describe, expect, it } from 'vitest';
import { FreshSessionRow } from '../chat-exchange-parts';
import { drawsResponseRow, getCollapsedVisibleEvents } from '../../../store/event-rendering';
import { exchangeResponseEvents } from '../../../store/thread-events';
import type { Exchange, StoredEvent } from '../../../store/thread-events';
import type { SessionContinuity } from '../../../generated/thread-event-wire';
import type { ResponseEvent } from '../../../store/types';

// A session that starts without the thread's earlier context says so, once, at
// the start of its turn. A first session and a resume draw nothing, and neither
// does an older event that predates the field.

function turnStartingWith(continuity?: SessionContinuity): Exchange {
  const started = { type: 'SessionStarted', session_id: 's1', channel: 'claude_code', ...(continuity ? { continuity } : {}) };
  return {
    userEvent: { type: 'MessageReceived', text: 'carry on', channel: 'claude_code' } as StoredEvent,
    userSeq: 0,
    steps: [
      { seq: 1, event: started as StoredEvent },
      { seq: 2, event: { type: 'CodingAgentTextStreamed', text: 'On it.' } as StoredEvent },
    ],
  };
}

const notes = (exchange: Exchange) =>
  exchangeResponseEvents(exchange).filter(e => e.type === 'fresh_session');

describe('a fresh session is noted in the thread', () => {
  it('draws the note, with its reason, before the turn says anything', () => {
    const events = exchangeResponseEvents(turnStartingWith({ kind: 'fresh', reason: 'branch-gone' }));
    const at = events.findIndex(e => e.type === 'fresh_session');
    expect(events[at]).toMatchObject({ type: 'fresh_session', reason: 'branch-gone' });
    expect(at).toBeLessThan(events.findIndex(e => e.type === 'text'));
  });

  it.each([
    ['a first session', { kind: 'first' } as const],
    ['a resumed session', { kind: 'resumed' } as const],
    ['an event that predates the field', undefined],
  ])('draws nothing for %s', (_, continuity) => {
    expect(notes(turnStartingWith(continuity))).toEqual([]);
  });

  it('draws with the steps control off, and survives the collapse', () => {
    const note: ResponseEvent = { type: 'fresh_session', reason: 'change-applied' };
    expect(drawsResponseRow(note, false)).toBe(true);
    const events: ResponseEvent[] = [note, { type: 'text', md: 'Earlier.' }, { type: 'text', md: 'Done.' }];
    expect(getCollapsedVisibleEvents(events)).toContainEqual(note);
  });
});

describe('the note reads as an event card', () => {
  it('names the new session, says the context did not carry over, and says why', () => {
    const s = JSON.stringify(FreshSessionRow({ event: { type: 'fresh_session', reason: 'branch-gone' } }));
    expect(s).toContain('New session');
    expect(s).toContain('Context not carried over');
    expect(s).toContain('The branch was renamed or deleted.');
    expect(s).toContain('Details');
  });
});
