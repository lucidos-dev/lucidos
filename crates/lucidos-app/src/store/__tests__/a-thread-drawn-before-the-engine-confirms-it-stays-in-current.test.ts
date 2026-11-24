import { describe, it, expect } from 'vitest';
import { makeOptimisticThreadState } from '../thread-events';
import { getThreadDisplaySection } from '../store';

describe('a thread drawn before the engine confirms it', () => {
  it('starts in the inbox, like the engine row', () => {
    const thread = makeOptimisticThreadState({
      id: 't1', title: '', channel: 'chat', initiator: 'user', eventsLoaded: true,
    });
    expect(thread.meta.section).toBe('inbox');
  });

  it('stays in Current after an unsent first send, with no summary yet', () => {
    const thread = makeOptimisticThreadState({
      id: 't1', title: '', channel: 'claude_code', initiator: 'user', eventsLoaded: true,
      state: 'composing', status: 'idle',
    });
    thread.meta.state = 'active';
    expect(thread.pendingUserMessages).toEqual([]);
    expect(getThreadDisplaySection(thread)).toBe('current');
  });
});
