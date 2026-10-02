/**
 * Regression: a completed thread's status dot stuck on "running" until reload,
 * and a stale read putting an idle status over a turn already running.
 *
 * A resync GET (`loadAllThreads` -> `upsertThread`, or `refreshThreadEvents`
 * -> `applyEventRows`) can fire before a live event and land after it. A
 * trigger bumps the summary's `summary_version` on every change to the row.
 * The client refuses any summary older than the one it holds.
 *
 * `last_activity` cannot order these reads. Several status changes do not
 * bump it, and the client holds engine time in `updatedAt`. So a timestamp
 * lets an equal-timestamp stale read through and refuses a fresh one. The
 * last two cases in each block pin both of those.
 */
import { describe, it, expect, beforeEach, vi } from 'vitest';

// Polyfill localStorage before store.ts is imported at module level.
vi.hoisted(() => {
  const storage = new Map<string, string>();
  (globalThis as any).localStorage = {
    getItem: (k: string) => storage.get(k) ?? null,
    setItem: (k: string, v: string) => storage.set(k, v),
    removeItem: (k: string) => storage.delete(k),
    clear: () => storage.clear(),
    get length() { return storage.size; },
    key: (_i: number) => null,
  };
  if (typeof globalThis.document === 'undefined') (globalThis as any).document = {};
  if (!(globalThis.document as any).querySelector) (globalThis.document as any).querySelector = () => null;
  if (!(globalThis.document as any).querySelectorAll) (globalThis.document as any).querySelectorAll = () => [];
  if (typeof globalThis.requestAnimationFrame === 'undefined') {
    (globalThis as any).requestAnimationFrame = (cb: any) => { cb(); return 0; };
  }
});

import { makeThreadState } from './threads-test-helpers';
import { applySummaryVersion, type ThreadAggregate, type ThreadState } from '../thread-events';
import type { ThreadSummary } from '../../api/threads';
import { focusedThreadId, threadMap } from '../store';
import { upsertThread, refreshThreadEvents } from './thread-loading';
import { composeSentAt } from './compose';
import { getDraft } from '../composeDrafts';

vi.mock('../../api/threads', () => ({
  fetchThreads: vi.fn(),
  fetchThreadEvents: vi.fn().mockResolvedValue({ events: [], currentAggregate: null }),
}));

const RUNNING_AT = '2026-06-15T16:36:48.000Z';
const IDLE_AT = '2026-06-15T16:36:51.571Z'; // a later, newer last_activity

function summary(overrides: Partial<ThreadSummary>): ThreadSummary {
  return {
    thread_id: 't1',
    title: 'Conductor AI Business Model',
    channel: 'chat',
    initiator: 'user',
    created_at: '2026-06-15T16:35:54.000Z',
    last_activity: IDLE_AT,
    message_count: 1,
    section: 'inbox',
    active_children_count: 0,
    total_children_count: 0,
    blocking_descendant_count: 0,
    attention_descendant_count: 0,
    live_event_wait_count: 0,
    status: 'idle',
    summary_version: 0,
    coding_agent_has_diff: false,
    coding_agent_proposed: false,
    coding_agent_requires_restart: false,
    coding_agent_is_external_repo: false,
    last_revived_at: null,
    state: 'active',
    compose_text: '',
    compose_images: [],
    ...overrides,
  };
}

function aggregate(overrides: Partial<ThreadAggregate>): ThreadAggregate {
  return {
    threadId: 't1',
    title: 'Conductor AI Business Model',
    channel: 'chat',
    initiator: 'user',
    createdAt: '2026-06-15T16:35:54.000Z',
    lastActivity: IDLE_AT,
    messageCount: 1,
    section: 'inbox',
    status: 'idle',
    summaryVersion: 0,
    activeChildrenCount: 0,
    totalChildrenCount: 0,
    blockingDescendantCount: 0,
    attentionDescendantCount: 0,
    liveEventWaitCount: 0,
    codingAgentHasDiff: false,
    codingAgentProposed: false,
    codingAgentRequiresRestart: false,
    codingAgentIsExternalRepo: false,
    isSaved: false,
    hasResponse: true,
    lastRevivedAt: null,
    parentThreadId: null,
    parentThreadTitle: null,
    state: 'active',
    ...overrides,
  };
}

/** The version the live state holds in every case below. */
const LIVE = 5;

/** A thread whose live meta holds `status` at version `LIVE`. */
function liveThread(overrides: Partial<ThreadState> & { status?: ThreadState['meta']['status'] } = {}): ThreadState {
  const { status = 'idle', ...rest } = overrides;
  const thread = makeThreadState('t1', { ...rest, meta: { id: 't1', updatedAt: IDLE_AT, ...rest.meta } });
  applySummaryVersion(thread.meta, LIVE, status);
  return thread;
}

beforeEach(() => {
  threadMap.value = new Map();
  focusedThreadId.value = null;
});

describe('upsertThread: the version guard (loadAllThreads path)', () => {
  it('does not regress a live idle to running when the GET snapshot is older', () => {
    const map = new Map([['t1', liveThread()]]);
    upsertThread(map, summary({ status: 'running', summary_version: LIVE - 1, last_activity: RUNNING_AT }), false);
    expect(map.get('t1')!.meta.status).toBe('idle');
  });

  it('applies status from a newer GET snapshot', () => {
    const map = new Map([['t1', liveThread()]]);
    upsertThread(map, summary({ status: 'running', summary_version: LIVE + 1 }), false);
    expect(map.get('t1')!.meta.status).toBe('running');
    expect(map.get('t1')!.meta.summaryVersion).toBe(LIVE + 1);
  });

  /** The lifecycle marker is the same kind of field and the same trap. An iOS
   *  wake fires `loadAllThreads`, the user presses Send while that GET is out,
   *  and the answer still says `composing`. Unguarded, the composer replaces
   *  the transcript the message just went into. */
  it('does not send an active thread back to composing on an older GET', () => {
    const map = new Map([['t1', liveThread({ meta: { id: 't1', state: 'active' } as ThreadState['meta'] })]]);
    upsertThread(map, summary({ state: 'composing', summary_version: LIVE - 1 }), false);
    expect(map.get('t1')!.meta.state).toBe('active');
  });

  /** The same race at an EQUAL version, which is the common one: nothing
   *  raises the version until the send's `MessageReceived` lands. The send's
   *  own stamp is what makes the snapshot stale. Unguarded, the composer came
   *  back holding the sent text, ready to be sent twice. */
  it('does not send a just-sent draft back to composing on a same-version GET from before the send', () => {
    const requestStartedAt = Date.now() - 50;
    const map = new Map([['t1', liveThread({ meta: { id: 't1', state: 'active' } as ThreadState['meta'] })]]);
    composeSentAt.set('t1', Date.now());
    try {
      upsertThread(map, summary({ state: 'composing', summary_version: LIVE, compose_text: 'hello' }), false, requestStartedAt);
      expect(map.get('t1')!.meta.state).toBe('active');
      expect(getDraft('t1').text).toBe('');
    } finally {
      composeSentAt.delete('t1');
    }
  });

  /** The guard must stay one-sided. A fresh snapshot is still what rescues an
   *  SSE skeleton stuck at `composing`, which no drawer section draws. */
  it('applies state from a newer GET snapshot', () => {
    const map = new Map([['t1', liveThread({ meta: { id: 't1', state: 'composing' } as ThreadState['meta'] })]]);
    upsertThread(map, summary({ state: 'active', summary_version: LIVE + 1 }), false);
    expect(map.get('t1')!.meta.state).toBe('active');
  });

  /** A Stop moves status without bumping `last_activity`, so the timestamp
   *  guard read this older snapshot as fresh and put `running` back. */
  it('refuses an older snapshot even when its last_activity ties the live one', () => {
    const map = new Map([['t1', liveThread()]]);
    upsertThread(map, summary({ status: 'running', summary_version: LIVE - 1, last_activity: IDLE_AT }), false);
    expect(map.get('t1')!.meta.status).toBe('idle');
  });

  /** `updatedAt` holds engine broadcast time, a later clock than the row's
   *  `last_activity`. The timestamp guard refused this genuinely newer read. */
  it('applies a newer snapshot even when its last_activity reads older', () => {
    const map = new Map([['t1', liveThread()]]);
    upsertThread(map, summary({ status: 'running', summary_version: LIVE + 1, last_activity: RUNNING_AT }), false);
    expect(map.get('t1')!.meta.status).toBe('running');
  });
});

describe('refreshThreadEvents: the version guard (applyEventRows path)', () => {
  async function refreshWith(agg: ThreadAggregate): Promise<ThreadState> {
    threadMap.value = new Map([['t1', liveThread({ eventsLoaded: true, lastDbSeq: 5 })]]);
    const { fetchThreadEvents } = await import('../../api/threads');
    (fetchThreadEvents as any).mockResolvedValue({ events: [], currentAggregate: agg });
    await refreshThreadEvents('t1');
    return threadMap.value.get('t1')!;
  }

  it('does not regress a live idle to running when the currentAggregate is older', async () => {
    const thread = await refreshWith(aggregate({ status: 'running', summaryVersion: LIVE - 1 }));
    expect(thread.meta.status).toBe('idle');
  });

  it('applies a newer currentAggregate that legitimately advances status', async () => {
    const thread = await refreshWith(aggregate({ status: 'running', summaryVersion: LIVE + 1 }));
    expect(thread.meta.status).toBe('running');
  });

  it('refuses an older currentAggregate even when its lastActivity ties the live one', async () => {
    const thread = await refreshWith(aggregate({ status: 'running', summaryVersion: LIVE - 1, lastActivity: IDLE_AT }));
    expect(thread.meta.status).toBe('idle');
  });

  it('applies a newer currentAggregate even when its lastActivity reads older', async () => {
    const thread = await refreshWith(aggregate({ status: 'running', summaryVersion: LIVE + 1, lastActivity: RUNNING_AT }));
    expect(thread.meta.status).toBe('running');
  });
});
