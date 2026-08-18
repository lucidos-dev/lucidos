/**
 * Regression: the subscription panel kept rendering an *event wait* the engine
 * had already resolved, reason text and live countdown and all.
 *
 * Root cause: `meta.liveEventWaits` was built ONLY by folding a thread's own
 * `EventWait*` events. `makeThreadState` seeded it to `[]` and `upsertThread`
 * never touched it again, and no API returned it on a summary, so no snapshot
 * could correct it. One missed `EventWaitDelivered` stranded the wait forever.
 * `handleEvent`'s seq-dedup guard returns before the fold, so a re-delivery of
 * the same sequence could not repair it either. The diagnostic signature was
 * the count reading 0 while the panel still drew the wait.
 *
 * Fix: `thread_summaries.live_event_waits`, written in the same statement as
 * `live_event_wait_count`, carried on both `ThreadSummary` and
 * `ThreadAggregate`, and overwritten by both snapshot paths.
 *
 * The load-bearing test is the first one: the client is deliberately never
 * handed the delivery. A test that fed it the resolution would prove only that
 * the fold works, which was never in doubt.
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
import { handleEvent, type EventWaitSummary, type ThreadAggregate, type ThreadEvent, type ThreadState } from '../thread-events';
import type { ThreadSummary } from '../../api/threads';
import { focusedThreadId, threadMap } from '../store';
import { upsertThread } from './thread-loading';

vi.mock('../../api/threads', () => ({
  fetchThreads: vi.fn(),
  fetchThreadEvents: vi.fn().mockResolvedValue({ events: [], currentAggregate: null }),
}));

/** When the wait was armed, and when the engine resolved it. Both are
 *  `thread_summaries.last_activity` values: the four `EventWait*` projection
 *  arms all bump it, so a resolution's snapshot is strictly newer. */
const ARMED_AT = '2026-08-14T10:00:00.000Z';
const RESOLVED_AT = '2026-08-14T10:07:00.000Z';

const WAIT: EventWaitSummary = {
  wait_id: 'w1',
  on: [{ event_type: 'BackgroundBashCompleted' }],
  reason: 'Waiting for Phase B to publish the draft',
  expires_at: '2026-08-14T18:00:00.000Z',
};

function summary(overrides: Partial<ThreadSummary>): ThreadSummary {
  return {
    thread_id: 't1',
    title: 'Release run',
    channel: 'chat',
    initiator: 'user',
    created_at: '2026-08-14T09:00:00.000Z',
    last_activity: RESOLVED_AT,
    message_count: 1,
    section: 'inbox',
    active_children_count: 0,
    total_children_count: 0,
    blocking_descendant_count: 0,
    attention_descendant_count: 0,
    live_event_wait_count: 0,
    live_event_waits: [],
    status: 'idle',
    coding_agent_has_diff: false,
    coding_agent_proposed: false,
    coding_agent_requires_restart: false,
    coding_agent_is_external_repo: false,
    coding_agent_applying: false,
    last_revived_at: null,
    state: 'active',
    compose_text: '',
    compose_images: [],
    ...overrides,
  };
}

/** A thread whose client folded in the arm and is still drawing the panel. */
function watchingThread(): Map<string, ThreadState> {
  const map = new Map<string, ThreadState>();
  map.set('t1', makeThreadState('t1', {
    meta: { id: 't1', updatedAt: ARMED_AT, liveEventWaitCount: 1, liveEventWaits: [WAIT] },
  }));
  return map;
}

beforeEach(() => {
  threadMap.value = new Map();
  focusedThreadId.value = null;
});

describe('upsertThread reconciles the live event-wait list against the server', () => {
  /** THE regression. The client never sees the `EventWaitDelivered`, exactly as
   *  in the report, so only the snapshot can save it. */
  it('drops a wait the client never saw resolved, on the next summary snapshot', () => {
    const map = watchingThread();

    upsertThread(map, summary({ live_event_wait_count: 0, live_event_waits: [], last_activity: RESOLVED_AT }), false);

    expect(map.get('t1')!.meta.liveEventWaits).toEqual([]);
    expect(map.get('t1')!.meta.liveEventWaitCount).toBe(0);
  });

  it('seeds the list from the snapshot for a thread entering the map', () => {
    const map = new Map<string, ThreadState>();

    upsertThread(map, summary({ live_event_wait_count: 1, live_event_waits: [WAIT] }), false);

    expect(map.get('t1')!.meta.liveEventWaits).toEqual([WAIT]);
  });

  it('applies a wait armed while this client was not looking', () => {
    const map = new Map<string, ThreadState>();
    map.set('t1', makeThreadState('t1', { meta: { id: 't1', updatedAt: ARMED_AT } }));

    upsertThread(map, summary({ live_event_wait_count: 1, live_event_waits: [WAIT], last_activity: RESOLVED_AT }), false);

    expect(map.get('t1')!.meta.liveEventWaits).toEqual([WAIT]);
  });

  /** A GET issued before the arm, landing after live SSE already applied it.
   *  Without the staleness guard this blanks a wait that is genuinely live. */
  it('does not blank a freshly armed wait when the GET snapshot is stale', () => {
    const map = new Map<string, ThreadState>();
    map.set('t1', makeThreadState('t1', {
      meta: { id: 't1', updatedAt: RESOLVED_AT, liveEventWaitCount: 1, liveEventWaits: [WAIT] },
    }));

    upsertThread(map, summary({ live_event_waits: [], last_activity: ARMED_AT }), false);

    expect(map.get('t1')!.meta.liveEventWaits).toEqual([WAIT]);
  });

  /** The mirror image: a GET issued before the delivery must not put a dead
   *  wait back on screen, counting down to a deadline nobody is waiting for. */
  it('does not resurrect a resolved wait when the GET snapshot is stale', () => {
    const map = new Map<string, ThreadState>();
    map.set('t1', makeThreadState('t1', { meta: { id: 't1', updatedAt: RESOLVED_AT } }));

    upsertThread(map, summary({ live_event_wait_count: 1, live_event_waits: [WAIT], last_activity: ARMED_AT }), false);

    expect(map.get('t1')!.meta.liveEventWaits).toEqual([]);
  });

  /** Absence is not emptiness. A partial fixture must leave a populated list
   *  alone, rather than silently clearing it and hiding a regression behind a
   *  passing test. */
  it('leaves the list alone when the snapshot omits the field', () => {
    const map = watchingThread();
    const partial = summary({ last_activity: RESOLVED_AT });
    delete (partial as Partial<ThreadSummary>).live_event_waits;

    upsertThread(map, partial, false);

    expect(map.get('t1')!.meta.liveEventWaits).toEqual([WAIT]);
  });
});

/** The per-event aggregate is the second reconciliation path, and the stronger
 *  one: `handleEvent` applies it BEFORE the `thread.events.has(seq)` dedup
 *  guard. So even a re-delivered sequence repairs a stranded list, which is the
 *  one path that could not repair it before. */
describe('handleEvent repairs the list from the per-event aggregate', () => {
  const aggregate = (waits: EventWaitSummary[]): ThreadAggregate => ({
    liveEventWaitCount: waits.length,
    liveEventWaits: waits,
    lastActivity: RESOLVED_AT,
  } as unknown as ThreadAggregate);

  const unrelated: ThreadEvent = { type: 'ResponseGenerated', text: 'done' } as ThreadEvent;

  it('drops a stranded wait on the next unrelated event', () => {
    const map = watchingThread();

    handleEvent(map, 't1', 42, unrelated, RESOLVED_AT, undefined, aggregate([]));

    expect(map.get('t1')!.meta.liveEventWaits).toEqual([]);
  });

  it('repairs the list even when the sequence is a re-delivery', () => {
    const map = watchingThread();
    handleEvent(map, 't1', 42, unrelated, RESOLVED_AT, undefined, aggregate([WAIT]));

    const result = handleEvent(map, 't1', 42, unrelated, RESOLVED_AT, undefined, aggregate([]));

    expect(result.applied).toBe(false);
    expect(result.metaChanged).toBe(true);
    expect(map.get('t1')!.meta.liveEventWaits).toEqual([]);
  });

  /** The aggregate lands first and the fold runs second, so the two must
   *  converge rather than double-count. `eventWaitProjection` is idempotent by
   *  `wait_id`, which is what makes that true. */
  it('does not duplicate a wait the aggregate already carries', () => {
    const map = new Map<string, ThreadState>();
    map.set('t1', makeThreadState('t1', { meta: { id: 't1', updatedAt: ARMED_AT } }));
    const armEvent: ThreadEvent = {
      type: 'EventWaitStarted',
      wait_id: WAIT.wait_id,
      tool_use_id: 'toolu_1',
      on: WAIT.on,
      reason: WAIT.reason,
      expires_at: WAIT.expires_at,
      watermark: 10,
    } as ThreadEvent;

    handleEvent(map, 't1', 43, armEvent, ARMED_AT, undefined, aggregate([WAIT]));

    expect(map.get('t1')!.meta.liveEventWaits).toEqual([WAIT]);
  });
});
