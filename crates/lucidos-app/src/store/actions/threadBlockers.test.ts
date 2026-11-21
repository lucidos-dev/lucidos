// The thread menu's Archive and Delete rows, per action blocker (ADR 0378).
// A blocker never removes a row: it shows the row blocked, with its reason and
// every sub-thread that blocks.

import { afterEach, describe, expect, it } from 'vitest';
import { threadMap } from '../store';
import type { ThreadState } from '../thread-events';
import { makeThread } from '../__tests__/thread-wiring-helpers';
import { BLOCKER_REASON, blockedBySubThreads, type HeldBlocker } from './blockerCopy';
import { exitItems, threadBlocker } from './threadBlockers';
import { collectThreadFamily } from './threadFamily';

type MetaPatch = Partial<ThreadState['meta']>;

function thread(id: string, meta: MetaPatch = {}): ThreadState {
  const base = makeThread();
  return { ...base, meta: { ...base.meta, id, section: 'inbox', ...meta } };
}

function seed(...threads: ThreadState[]): void {
  threadMap.value = new Map(threads.map((t) => [t.meta.id, t]));
}

afterEach(() => {
  threadMap.value = new Map();
});

const waiting: MetaPatch = { status: 'waiting_for_user_answer' };
const pending: MetaPatch = { channel: 'claude_code', codingAgentChangeState: { kind: 'proposed', requires_restart: false }};

/** One family per blocker: the target is `t`, a sub-thread is `c`. */
const FAMILIES: { blocker: HeldBlocker; threads: ThreadState[] }[] = [
  { blocker: 'home', threads: [thread('t', { home: true })] },
  { blocker: 'running', threads: [thread('t', { status: 'running' })] },
  { blocker: 'question', threads: [thread('t', waiting)] },
  { blocker: 'pending_change', threads: [thread('t', pending)] },
  {
    blocker: 'descendant_running',
    threads: [thread('t'), thread('c', { parentThreadId: 't', status: 'running' })],
  },
  {
    blocker: 'descendant_question',
    threads: [thread('t'), thread('c', { parentThreadId: 't', ...waiting })],
  },
  {
    blocker: 'descendant_pending_change',
    threads: [thread('t'), thread('c', { parentThreadId: 't', ...pending })],
  },
];

describe('exitItems', () => {
  it('offers both exits on a thread nothing blocks', () => {
    seed(thread('t'));
    expect(exitItems('t')).toEqual({ archive: 'enabled', unarchive: false, delete: 'enabled', reason: null, subThreads: [] });
  });

  for (const { blocker, threads } of FAMILIES) {
    it(`shows both exits blocked, with the reason, for ${blocker}`, () => {
      seed(...threads);
      const items = exitItems('t');
      expect(items.archive).toBe('blocked');
      expect(items.delete).toBe('blocked');
      const descendant = blocker.startsWith('descendant_');
      expect(items.reason).toBe(descendant ? blockedBySubThreads(1) : BLOCKER_REASON[blocker]);
      expect(items.subThreads.map((s) => s.thread.meta.id)).toEqual(descendant ? ['c'] : []);
    });
  }

  it('trades Archive for Move to Current on an archived thread, and keeps Delete', () => {
    seed(thread('t', { section: 'archived' }));
    expect(exitItems('t')).toMatchObject({ archive: null, unarchive: true, delete: 'enabled' });
  });

  it('keeps a stored-archived thread its pending change holds in Current, saying why', () => {
    // The change keeps it in Current, so Archive is shown blocked, never Move to Current.
    seed(thread('t', { section: 'archived', ...pending }));
    expect(exitItems('t')).toMatchObject({
      archive: 'blocked', unarchive: false, delete: 'blocked', reason: BLOCKER_REASON.pending_change,
    });
  });

  it('offers Archive on a stored-archived thread a stale count keeps in Current', () => {
    // A tester's coordinator: stored archived, every sub-thread archived and idle,
    // shown in Current by a drifted active count. Archive makes the engine recount.
    seed(
      thread('t', { section: 'archived', activeChildrenCount: 12 }),
      thread('c', { parentThreadId: 't', section: 'archived' }),
    );
    expect(exitItems('t')).toMatchObject({ archive: 'enabled', unarchive: false, delete: 'enabled', reason: null });
  });

  it('lets an archived sub-thread holding a change through, as the engine does', () => {
    seed(thread('t'), thread('c', { parentThreadId: 't', section: 'archived', ...pending }));
    expect(exitItems('t').archive).toBe('enabled');
  });

  it('draws no exit on a draft', () => {
    seed(thread('t', { state: 'composing' }));
    expect(exitItems('t')).toMatchObject({ archive: null, unarchive: false, delete: null });
  });

  it('leaves the exits enabled when the count says a sub-thread blocks but none is loaded', () => {
    // The engine's 409 then names the blocker; the client never guesses it.
    seed(thread('t', { blockingDescendantCount: 1 }));
    expect(exitItems('t')).toMatchObject({ archive: 'enabled', delete: 'enabled' });
  });
});

/** The blocking sub-threads as `id:blocker`, in the order the menu lists them. */
function listed(threadId: string): string[] {
  return threadBlocker(threadId).subThreads.map((s) => `${s.thread.meta.id}:${s.blocker}`);
}

describe('threadBlocker', () => {
  it('names the strongest sub-thread reason and lists every blocking sub-thread, strongest first', () => {
    seed(
      thread('t'),
      thread('q', { parentThreadId: 't', ...waiting }),
      thread('grandchild', { parentThreadId: 'q', status: 'running' }),
      thread('idle', { parentThreadId: 't' }),
    );
    expect(threadBlocker('t').blocker).toBe('descendant_running');
    expect(listed('t')).toEqual(['grandchild:running', 'q:question']);
  });

  it('keeps the family order among sub-threads with the same blocker', () => {
    seed(
      thread('t'),
      thread('a', { parentThreadId: 't', ...pending }),
      thread('b', { parentThreadId: 't', ...pending }),
      thread('c', { parentThreadId: 't', ...pending }),
      thread('d', { parentThreadId: 't', ...pending }),
    );
    const order = [...collectThreadFamily('t')].filter((id) => id !== 't');
    expect(listed('t')).toEqual(order.map((id) => `${id}:pending_change`));
    expect(exitItems('t').reason).toBe(blockedBySubThreads(4));
  });

  it("puts the thread's own blocker before any sub-thread's, and lists none", () => {
    seed(thread('t', pending), thread('c', { parentThreadId: 't', status: 'running' }));
    expect(threadBlocker('t')).toEqual({ blocker: 'pending_change', subThreads: [] });
  });

  it('exempts an external-repo coding agent from the pending-change blocker', () => {
    seed(thread('t', { ...pending, codingAgentIsExternalRepo: true }));
    expect(threadBlocker('t').blocker).toBe('none');
  });
});
