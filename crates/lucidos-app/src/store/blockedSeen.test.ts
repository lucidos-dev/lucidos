import { beforeEach, describe, expect, it } from 'vitest';
import { hasUnseenBlocked, seenBlockedAfter } from './blockedSeen';
import { focusedThreadId, setDrawerGrouping, setSelectedOngoingGroup, threadMap } from './store';
import { makeThreadState } from './actions/threads-test-helpers';
import type { ThreadStatus } from './thread-events';

const ids = (...xs: string[]) => new Set(xs);

describe('seenBlockedAfter', () => {
  it('listing the Blocked group sees every blocked thread', () => {
    expect(seenBlockedAfter(ids(), ids('a', 'b'), 'all')).toEqual(ids('a', 'b'));
  });

  it('opening a blocked thread sees only that one', () => {
    expect(seenBlockedAfter(ids(), ids('a', 'b'), 'a')).toEqual(ids('a'));
  });

  it('opening a thread that is not blocked sees nothing', () => {
    expect(seenBlockedAfter(ids(), ids('a'), 'z')).toEqual(ids());
  });

  it('a thread that stops being blocked is forgotten, so a new block is unseen', () => {
    expect(seenBlockedAfter(ids('a', 'b'), ids('b'), null)).toEqual(ids('b'));
  });
});

function blocked(id: string, status: ThreadStatus = 'waiting_for_user_answer') {
  return makeThreadState(id, { meta: { status, section: 'inbox' } });
}

describe('hasUnseenBlocked', () => {
  beforeEach(() => {
    focusedThreadId.value = null;
    setDrawerGrouping('ongoing');
    setSelectedOngoingGroup('review');
    threadMap.value = new Map();
  });

  it('holds for a new blocked thread, and stops once the Blocked group lists', () => {
    threadMap.value = new Map([['a', blocked('a')]]);
    expect(hasUnseenBlocked.value).toBe(true);
    setSelectedOngoingGroup('blocked');
    expect(hasUnseenBlocked.value).toBe(false);
  });

  it('moving off the Blocked group does not restart it for threads already seen', () => {
    threadMap.value = new Map([['a', blocked('a')]]);
    setSelectedOngoingGroup('blocked');
    setSelectedOngoingGroup('review');
    expect(hasUnseenBlocked.value).toBe(false);
  });

  it('a newly blocked thread restarts it', () => {
    threadMap.value = new Map([['a', blocked('a')]]);
    setSelectedOngoingGroup('blocked');
    setSelectedOngoingGroup('review');
    threadMap.value = new Map([['a', blocked('a')], ['b', blocked('b')]]);
    expect(hasUnseenBlocked.value).toBe(true);
  });

  it('opening the blocked thread sees it', () => {
    threadMap.value = new Map([['a', blocked('a')]]);
    focusedThreadId.value = 'a';
    expect(hasUnseenBlocked.value).toBe(false);
  });

  it('a thread that unblocks and blocks again is unseen again', () => {
    threadMap.value = new Map([['a', blocked('a')]]);
    focusedThreadId.value = 'a';
    focusedThreadId.value = null;
    threadMap.value = new Map([['a', blocked('a', 'idle')]]);
    threadMap.value = new Map([['a', blocked('a')]]);
    expect(hasUnseenBlocked.value).toBe(true);
  });
});
