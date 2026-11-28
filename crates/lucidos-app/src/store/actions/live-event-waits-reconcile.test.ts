import { describe, it, expect, beforeEach, vi } from 'vitest';

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
import { applySummaryVersion, handleEvent, type EventWaitSummary, type ThreadAggregate, type ThreadEvent, type ThreadState } from '../thread-events';
import type { ThreadSummary } from '../../api/threads';
import { focusedThreadId, threadMap } from '../store';
import { upsertThread } from './thread-loading';

vi.mock('../../api/threads', () => ({
  fetchThreads: vi.fn(),
  fetchThreadEvents: vi.fn().mockResolvedValue({ events: [], currentAggregate: null }),
}));

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
    archivable_sub_thread_count: 0,
    live_event_wait_count: 0,
    live_event_waits: [],
    status: 'idle',
    summary_version: 0,
    coding_agent_change_state: { kind: 'none' },
    coding_agent_is_external_repo: false,
    last_revived_at: null,
    state: 'active',
    compose_text: '',
    compose_images: [],
    ...overrides,
  };
}

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

  it('does not blank a freshly armed wait when the GET snapshot is older', () => {
    const map = new Map<string, ThreadState>();
    const thread = makeThreadState('t1', {
      meta: { id: 't1', updatedAt: RESOLVED_AT, liveEventWaitCount: 1, liveEventWaits: [WAIT] },
    });
    applySummaryVersion(thread.meta, 5, 'idle');
    map.set('t1', thread);

    upsertThread(map, summary({ live_event_waits: [], summary_version: 4, last_activity: ARMED_AT }), false);

    expect(map.get('t1')!.meta.liveEventWaits).toEqual([WAIT]);
  });

  it('does not resurrect a resolved wait when the GET snapshot is older', () => {
    const map = new Map<string, ThreadState>();
    const thread = makeThreadState('t1', { meta: { id: 't1', updatedAt: RESOLVED_AT } });
    applySummaryVersion(thread.meta, 5, 'idle');
    map.set('t1', thread);

    upsertThread(map, summary({ live_event_wait_count: 1, live_event_waits: [WAIT], summary_version: 4, last_activity: ARMED_AT }), false);

    expect(map.get('t1')!.meta.liveEventWaits).toEqual([]);
  });

  it('leaves the list alone when the snapshot omits the field', () => {
    const map = watchingThread();
    const partial = summary({ last_activity: RESOLVED_AT });
    delete (partial as Partial<ThreadSummary>).live_event_waits;

    upsertThread(map, partial, false);

    expect(map.get('t1')!.meta.liveEventWaits).toEqual([WAIT]);
  });
});

describe('handleEvent repairs the list from the per-event aggregate', () => {
  const aggregate = (waits: EventWaitSummary[]): ThreadAggregate => ({
    liveEventWaitCount: waits.length,
    liveEventWaits: waits,
    lastActivity: RESOLVED_AT,
    codingAgentChangeState: { kind: 'none' },
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
