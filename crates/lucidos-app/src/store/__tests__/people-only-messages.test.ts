/** People-only messages and the agent pause in the transcript (ADR 0649).
 *
 *  A `PeopleMessagePosted` reads as a typed message but starts no turn, and
 *  an `AgentPaused` or `AgentResumed` is a note. All three are turnless
 *  boundaries: a turn already running keeps writing where it was. */
import { beforeEach, describe, expect, it } from 'vitest';
import { getExchanges, insertEvents, makeThread, resetSeqCounter } from './thread-flows-helpers';
import { exchangeStatus, exchangeUserImageHashes, exchangeUserMessage, isTurnlessBoundary, isUserBubbleEvent, type ThreadEvent } from '../thread-events';

beforeEach(resetSeqCounter);

const SAM = { kind: 'device', device_id: 'sam-phone' } as const;

const paused = { type: 'AgentPaused', cause: 'mention', actor: SAM } as ThreadEvent;
const resumed = { type: 'AgentResumed', cause: 'chip', actor: SAM } as ThreadEvent;
const peopleMessage = (text: string) => ({
  type: 'PeopleMessagePosted',
  text,
  user_image_hashes: ['abc'],
  device_id: 'sam-phone',
  mentioned_operator_ids: ['kim'],
  actor: SAM,
}) as ThreadEvent;

describe('a people-only message and the pause in the transcript', () => {
  it('are turnless boundaries, and the message draws as a bubble', () => {
    for (const event of [paused, resumed, peopleMessage('hi')]) {
      expect(isTurnlessBoundary(event)).toBe(true);
    }
    expect(isUserBubbleEvent(peopleMessage('hi'))).toBe(true);
    expect(isUserBubbleEvent(paused)).toBe(false);
  });

  it('opens one done exchange each, carrying the words and the images', () => {
    const { map, id } = makeThread();
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'draft the release note' },
      { type: 'ResponseGenerated', text: 'Drafted.' },
      paused,
      peopleMessage('Kim, can you check the numbers?'),
      resumed,
    ] as ThreadEvent[]);

    const exchanges = getExchanges(map, id);
    expect(exchanges.map((e) => e.userEvent.type)).toEqual([
      'MessageReceived', 'AgentPaused', 'PeopleMessagePosted', 'AgentResumed',
    ]);
    const message = exchanges[2];
    expect(exchangeUserMessage(message)).toBe('Kim, can you check the numbers?');
    expect(exchangeUserImageHashes(message)).toEqual(['abc']);
    for (const note of exchanges.slice(1)) {
      expect(exchangeStatus(note, '', true, false, false, true)).toBe('done');
    }
  });

  /** Pausing never alters a running turn, and a people-only message is never
   *  part of one: the turn's work keeps landing in its own exchange. */
  it('leaves a running turn owning its own work', () => {
    const { map, id } = makeThread('thread-1', 'running');
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'look into the outage' },
      { type: 'ToolCalled', name: 'read_file', args: { path: 'log.md' } },
      paused,
      peopleMessage('Sam here, I think it is the cache'),
      { type: 'TodoListWritten', items: [{ content: 'check', status: 'in_progress' }] },
    ] as ThreadEvent[]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(3);
    expect(exchanges[0].steps.map((s) => s.event.type)).toEqual(['ToolCalled', 'TodoListWritten']);
    expect(exchanges[1].steps).toHaveLength(0);
    expect(exchanges[2].steps).toHaveLength(0);
  });

  it('settles the optimistic row its send put up', () => {
    const { map, id } = makeThread();
    const thread = map.get(id)!;
    thread.pendingUserMessages.push({ eventId: 'evt-1', text: 'Kim, look', timestamp: '2026-11-10T10:00:00Z' } as never);
    insertEvents(map, id, [{ ...peopleMessage('Kim, look'), event_id: 'evt-1' }] as never);
    expect(thread.pendingUserMessages).toHaveLength(0);
  });
});
