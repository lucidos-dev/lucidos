/** Users-only messages and the agent pause in the transcript (ADR 0649, 0657).
 *
 *  A `UserOnlyMessagePosted` reads as a typed message but starts no turn, so it
 *  is a turnless boundary: a turn already running keeps writing where it was.
 *  An `AgentPaused` or `AgentResumed` draws nothing and opens no exchange. */
import { beforeEach, describe, expect, it } from 'vitest';
import { getExchanges, insertEvents, makeThread, resetSeqCounter } from './thread-flows-helpers';
import { exchangeStatus, exchangeUserImageHashes, exchangeUserMessage, isTurnlessBoundary, isUserBubbleEvent, type ThreadEvent } from '../thread-events';

beforeEach(resetSeqCounter);

const SAM = { kind: 'device', device_id: 'sam-phone' } as const;

const paused = { type: 'AgentPaused', cause: 'mention', actor: SAM } as ThreadEvent;
const resumed = { type: 'AgentResumed', cause: 'chip', actor: SAM } as ThreadEvent;
const userOnlyMessage = (text: string) => ({
  type: 'UserOnlyMessagePosted',
  text,
  user_image_hashes: ['abc'],
  device_id: 'sam-phone',
  mentioned_user_ids: ['kim'],
  actor: SAM,
}) as ThreadEvent;

describe('a user-only message and the pause in the transcript', () => {
  it('is a turnless boundary, and the message draws as a bubble', () => {
    expect(isTurnlessBoundary(userOnlyMessage('hi'))).toBe(true);
    expect(isUserBubbleEvent(userOnlyMessage('hi'))).toBe(true);
    expect(isUserBubbleEvent(paused)).toBe(false);
  });

  it('opens one done exchange per message, carrying the words and the images', () => {
    const { map, id } = makeThread();
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'draft the release note' },
      { type: 'ResponseGenerated', text: 'Drafted.' },
      paused,
      userOnlyMessage('Kim, can you check the numbers?'),
      resumed,
    ] as ThreadEvent[]);

    const exchanges = getExchanges(map, id);
    expect(exchanges.map((e) => e.userEvent.type)).toEqual(['MessageReceived', 'UserOnlyMessagePosted']);
    const message = exchanges[1];
    expect(exchangeUserMessage(message)).toBe('Kim, can you check the numbers?');
    expect(exchangeUserImageHashes(message)).toEqual(['abc']);
    expect(exchangeStatus(message, '', true, false, false, true)).toBe('done');
  });

  /** A pause draws nothing and never splits a running turn, and a user-only
   *  message is never part of one: the turn's work keeps landing in its own
   *  exchange. */
  it('leaves a running turn owning its own work', () => {
    const { map, id } = makeThread('thread-1', 'running');
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'look into the outage' },
      { type: 'ToolCalled', name: 'read_file', args: { path: 'log.md' } },
      paused,
      userOnlyMessage('Sam here, I think it is the cache'),
      resumed,
      { type: 'TodoListWritten', items: [{ content: 'check', status: 'in_progress' }] },
    ] as ThreadEvent[]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(2);
    expect(exchanges[0].steps.map((s) => s.event.type)).toEqual(['ToolCalled', 'TodoListWritten']);
    expect(exchanges[1].steps).toHaveLength(0);
  });

  it('settles the optimistic row its send put up', () => {
    const { map, id } = makeThread();
    const thread = map.get(id)!;
    thread.pendingUserMessages.push({ eventId: 'evt-1', text: 'Kim, look', timestamp: '2026-11-10T10:00:00Z' } as never);
    insertEvents(map, id, [{ ...userOnlyMessage('Kim, look'), event_id: 'evt-1' }] as never);
    expect(thread.pendingUserMessages).toHaveLength(0);
  });
});
