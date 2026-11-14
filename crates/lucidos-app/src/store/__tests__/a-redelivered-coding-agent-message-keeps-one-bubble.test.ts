/**
 * A coding agent's process can die holding a follow-up it never read. The next
 * session forwards it again, which records a second `CodingAgentPromptSent`
 * for the same message. The transcript still shows one bubble, queued until
 * the agent reads it or the user removes it. See
 * docs/plans/2026-10-17-a-follow-up-a-dead-agent-never-read.md.
 */
import { describe, it, expect } from 'vitest';
import {
  computeExchanges,
  makeOptimisticThreadState,
  type Exchange,
  type StoredEvent,
  type ThreadEvent,
} from '../thread-events';

function ev(seq: number, e: object, id?: string): readonly [number, StoredEvent] {
  const created = `2026-10-17T09:00:${String(seq).padStart(2, '0')}Z`;
  return [seq, { ...e, created, ...(id ? { _eventId: id } : {}) } as StoredEvent] as const;
}

const message = (text: string) => ({ type: 'MessageReceived', text, channel: 'claude_code' });
const said = (text: string) => ({ type: 'CodingAgentTextStreamed', text, channel: 'claude_code' });
const read = (id: string): ThreadEvent => ({ type: 'CodingAgentInputRead', input_event_id: id });
const forwarded = (text: string, id: string) =>
  ({ type: 'CodingAgentPromptSent', text, input_event_id: id, channel: 'claude_code' });

const FOLLOW_UP = 'also add a test';

/** The reported order: the first process takes the follow-up into its queue
 *  and dies, and the next session resumes the turn and forwards it again. */
const redelivered = [
  ev(1, message('build it'), 'm1'),
  ev(2, read('m1')),
  ev(3, said('working on it')),
  ev(4, message(FOLLOW_UP), 'm2'),
  ev(5, forwarded(FOLLOW_UP, 'm2')),
  ev(6, { type: 'ContinuationRequested', reason: 'answered_after_idle', channel: 'claude_code' }),
  ev(7, { type: 'SessionStarted', session_id: '', channel: 'claude_code' }),
  ev(8, said('picking up where I left off')),
  ev(9, forwarded(FOLLOW_UP, 'm2')),
];

function exchangesOf(events: ReadonlyArray<readonly [number, StoredEvent]>, live: boolean): Exchange[] {
  const thread = makeOptimisticThreadState({
    id: 't1',
    title: 'A coding-agent thread',
    channel: 'claude_code',
    initiator: 'user',
    eventsLoaded: true,
  });
  for (const [seq, event] of events) {
    thread.events.set(seq, event);
    if (live) computeExchanges(thread);
  }
  return computeExchanges(thread);
}

const bubbles = (exchanges: Exchange[]) =>
  exchanges.filter(ex => ex.userEvent.type === 'MessageReceived' && ex.userEvent.text === FOLLOW_UP);
const texts = (ex: Exchange) =>
  ex.steps.flatMap(s => s.event.type === 'CodingAgentTextStreamed' ? [s.event.text] : []);

describe.each([
  ['in one load', false],
  ['as each event arrives', true],
])('a redelivered coding-agent message, %s', (_, live) => {
  it('keeps one bubble, still queued, before the agent reads it', () => {
    const exchanges = exchangesOf(redelivered, live);
    expect(bubbles(exchanges)).toHaveLength(1);
    const [queued] = bubbles(exchanges);
    expect(queued.awaitingRead).toBe(true);
    expect(queued.steps).toEqual([]);
    const running = exchanges[0];
    expect(running.steps.filter(s => s.event.type === 'CodingAgentPromptSent')).toEqual([]);
  });

  it('leaves the queue on the late read and takes the steps after it', () => {
    const exchanges = exchangesOf([...redelivered, ev(10, read('m2')), ev(11, said('adding the test'))], live);
    expect(bubbles(exchanges)).toHaveLength(1);
    const [answered] = bubbles(exchanges);
    expect(answered.awaitingRead).toBeUndefined();
    expect(texts(answered)).toEqual(['adding the test']);
  });

  it('leaves the transcript when the user removes it', () => {
    const exchanges = exchangesOf(
      [...redelivered.slice(0, 5), ev(6, { type: 'QueuedMessageRemoved', removed_message_id: 'm2' })],
      live,
    );
    expect(bubbles(exchanges)).toEqual([]);
  });
});
