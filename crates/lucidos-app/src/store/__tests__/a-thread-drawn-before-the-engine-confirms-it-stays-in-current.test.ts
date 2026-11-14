/** A thread this device drew before any engine summary sits in Current.
 *
 *  The engine creates every thread row in the inbox, so the optimistic row
 *  must too. With an archive default, only a `running` status held the row in
 *  Current. A first send that got no answer drops its pending row, so the
 *  status fell back to the draft's `idle`. The thread then left Current for
 *  the Archive section until a summary corrected it.
 */
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
    // `sendCompose` flips the draft live; the unsent outcome then removes the
    // pending row and leaves no turn running.
    thread.meta.state = 'active';
    expect(thread.pendingUserMessages).toEqual([]);
    expect(getThreadDisplaySection(thread)).toBe('current');
  });
});
