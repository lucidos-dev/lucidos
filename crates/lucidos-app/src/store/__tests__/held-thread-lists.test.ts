import { describe, expect, it } from 'vitest';
import { makeThread } from './thread-flows-helpers';
import { listsInOngoing, threadInReview, threadIsBlocked } from '../store';
import { threadIsInFlight } from '../../components/drawer/family-graph';
import type { ThreadState } from '../thread-events';

function liveThread(status: ThreadState['meta']['status'] = 'idle'): ThreadState {
  const { map, id } = makeThread('held', status);
  const thread = map.get(id)!;
  thread.meta.section = 'inbox';
  return thread;
}

function hold(thread: ThreadState): void {
  thread.meta.hold = { reason: 'Waiting on the vendor', root_thread_id: thread.meta.id, placed_at: '2026-04-12T00:00:00Z' };
}

describe('a thread on hold and the Ongoing tiles', () => {
  it('lists a live thread until it is held', () => {
    const thread = liveThread();
    expect(listsInOngoing(thread)).toBe(true);
    hold(thread);
    expect(listsInOngoing(thread)).toBe(false);
  });

  it('keeps a saved thread off the tiles while it is held', () => {
    const thread = liveThread();
    thread.meta.saved = true;
    hold(thread);
    expect(listsInOngoing(thread)).toBe(false);
  });

  it('takes a failed thread out of Blocked', () => {
    const thread = liveThread('failed');
    expect(threadIsBlocked(thread)).toBe(true);
    hold(thread);
    expect(threadIsBlocked(thread)).toBe(false);
  });

  it('takes a read request out of Review', () => {
    const thread = liveThread();
    thread.meta.readRequested = true;
    expect(threadInReview(thread)).toBe(true);
    hold(thread);
    expect(threadInReview(thread)).toBe(false);
  });

  it('takes a running thread out of In flight', () => {
    const thread = liveThread('running');
    expect(threadIsInFlight(thread)).toBe(true);
    hold(thread);
    expect(threadIsInFlight(thread)).toBe(false);
  });
});
