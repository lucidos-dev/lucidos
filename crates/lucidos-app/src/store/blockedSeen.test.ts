import { beforeEach, describe, expect, it, vi } from 'vitest';
import { hasUnseenBlocked, seenBlockedAfter } from './blockedSeen';
import { focusedThreadId, setDrawerGrouping, setSelectedOngoingGroup, threadListFetched, threadMap } from './store';
import { makeThreadState } from './actions/threads-test-helpers';
import type { ThreadStatus } from './thread-events';

const keys = (...xs: string[]) => new Set(xs);
/** Blocked threads mapped to their block keys, here `<id>1`. */
const blockedMap = (...ids: string[]) => new Map(ids.map(id => [id, `${id}1`]));

describe('seenBlockedAfter', () => {
  it('listing the Blocked group sees every blocked thread', () => {
    expect(seenBlockedAfter(keys(), blockedMap('a', 'b'), 'all')).toEqual(keys('a1', 'b1'));
  });

  it('opening a blocked thread sees only that one', () => {
    expect(seenBlockedAfter(keys(), blockedMap('a', 'b'), 'a')).toEqual(keys('a1'));
  });

  it('opening a thread that is not blocked sees nothing', () => {
    expect(seenBlockedAfter(keys(), blockedMap('a'), 'z')).toEqual(keys());
  });

  it('a block that ends is forgotten, so a new block is unseen', () => {
    expect(seenBlockedAfter(keys('a1', 'b1'), blockedMap('b'), null)).toEqual(keys('b1'));
  });

  it('a seen key from an earlier block does not see the current one', () => {
    expect(seenBlockedAfter(keys('a0'), blockedMap('a'), null)).toEqual(keys());
  });
});

function blocked(id: string, status: ThreadStatus = 'waiting_for_user_answer', lastAgentAction = 'T1') {
  return makeThreadState(id, { meta: { status, section: 'inbox', lastAgentAction } });
}

describe('hasUnseenBlocked', () => {
  beforeEach(() => {
    focusedThreadId.value = null;
    setDrawerGrouping('ongoing');
    setSelectedOngoingGroup('review');
    threadListFetched.value = true;
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

  it('a follow-up from another device while still blocked keeps it seen', () => {
    threadMap.value = new Map([['a', blocked('a')]]);
    focusedThreadId.value = 'a';
    focusedThreadId.value = null;
    const followedUp = blocked('a');
    followedUp.meta.lastUserAction = 'T9';
    threadMap.value = new Map([['a', followedUp]]);
    expect(hasUnseenBlocked.value).toBe(false);
  });
});

describe('hasUnseenBlocked across a reload', () => {
  beforeEach(() => {
    localStorage.clear();
    vi.resetModules();
  });

  async function freshPage() {
    const store = await import('./store');
    const { hasUnseenBlocked } = await import('./blockedSeen');
    store.setDrawerGrouping('ongoing');
    store.setSelectedOngoingGroup('review');
    return { store, hasUnseenBlocked };
  }

  /** Sees blocked thread `a`, closes it, and reloads onto a list where `a` is
   *  blocked after the given last agent action. */
  async function reloadAfterSeeing(lastAgentActionAfter: string) {
    const before = await freshPage();
    before.store.threadListFetched.value = true;
    before.store.threadMap.value = new Map([['a', blocked('a')]]);
    before.store.setFocusedThread('a');
    expect(before.hasUnseenBlocked.value).toBe(false);
    before.store.setFocusedThread(null);

    vi.resetModules();
    const after = await freshPage();
    expect(after.hasUnseenBlocked.value).toBe(false);
    // The loader's order: the list lands, then it counts as fetched.
    after.store.threadMap.value = new Map([['a', blocked('a', 'waiting_for_user_answer', lastAgentActionAfter)]]);
    after.store.threadListFetched.value = true;
    return after.hasUnseenBlocked.value;
  }

  it('a thread seen before the reload stays seen after it', async () => {
    expect(await reloadAfterSeeing('T1')).toBe(false);
  });

  it('a block that began while the page was closed is unseen', async () => {
    expect(await reloadAfterSeeing('T2')).toBe(true);
  });
});
