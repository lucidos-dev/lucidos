import { describe, it, expect, beforeEach, vi } from 'vitest';
import { threadMap } from '../store';
import { makeThreadState } from './threads-test-helpers';

const loadThreadEvents = vi.hoisted(() => vi.fn());
vi.mock('./thread-loading', async (importOriginal) => ({
  ...(await importOriginal<typeof import('./thread-loading')>()),
  loadThreadEvents,
}));

import { rebuildCorruptedThreadEvents } from './thread-event-repair';

beforeEach(() => {
  loadThreadEvents.mockReset().mockResolvedValue(undefined);
  threadMap.value = new Map();
});

describe('rebuildCorruptedThreadEvents', () => {
  it('replaces the events Map with a fresh copy rather than reusing the long-lived one', () => {
    const thread = makeThreadState('t-1');
    const before = thread.events;
    threadMap.value = new Map([['t-1', thread]]);
    rebuildCorruptedThreadEvents('t-1');
    expect(thread.events).not.toBe(before);
    expect(thread.events).toEqual(before);
  });

  it('forces a full re-fetch of the thread events', () => {
    threadMap.value = new Map([['t-1', makeThreadState('t-1')]]);
    rebuildCorruptedThreadEvents('t-1');
    expect(loadThreadEvents).toHaveBeenCalledWith('t-1');
  });
});
