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
import { applySummaryVersion, type ThreadAggregate, type ThreadState } from '../thread-events';
import type { ThreadSummary } from '../../api/threads';
import { focusedThreadId, threadMap } from '../store';
import { upsertThread, refreshThreadEvents } from './thread-loading';
import { noteLocalComposeEdit, _resetComposeSyncForTesting } from '../composeSync';
import { getDraft } from '../composeDrafts';

vi.mock('../../api/threads', () => ({
  fetchThreads: vi.fn(),
  fetchThreadEvents: vi.fn().mockResolvedValue({ events: [], currentAggregate: null }),
}));

const RUNNING_AT = '2026-06-15T16:36:48.000Z';
const IDLE_AT = '2026-06-15T16:36:51.571Z';

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
    coding_agent_change_state: { kind: 'none' },
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
    codingAgentChangeState: { kind: 'none' },
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

const LIVE = 5;

function liveThread(overrides: Partial<ThreadState> & { status?: ThreadState['meta']['status'] } = {}): ThreadState {
  const { status = 'idle', ...rest } = overrides;
  const thread = makeThreadState('t1', { ...rest, meta: { id: 't1', updatedAt: IDLE_AT, ...rest.meta } });
  applySummaryVersion(thread.meta, LIVE, status);
  return thread;
}

beforeEach(() => {
  threadMap.value = new Map();
  focusedThreadId.value = null;
  _resetComposeSyncForTesting();
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

  it('does not send an active thread back to composing on an older GET', () => {
    const map = new Map([['t1', liveThread({ meta: { id: 't1', state: 'active' } as ThreadState['meta'] })]]);
    upsertThread(map, summary({ state: 'composing', summary_version: LIVE - 1 }), false);
    expect(map.get('t1')!.meta.state).toBe('active');
  });

  it('does not send a just-sent draft back to composing on a same-version GET from before the send', () => {
    const map = new Map([['t1', liveThread({
      meta: { id: 't1', state: 'active' } as ThreadState['meta'],
      pendingUserMessages: [{ text: 'hello', eventId: 'e1', created: IDLE_AT }],
    })]]);
    noteLocalComposeEdit('t1');
    upsertThread(map, summary({ state: 'composing', summary_version: LIVE, compose_text: 'hello' }), false);
    expect(map.get('t1')!.meta.state).toBe('active');
    expect(getDraft('t1').text).toBe('');
  });

  it('does not restage the pre-send draft while the send is still pending', () => {
    const map = new Map([['t1', liveThread({
      meta: { id: 't1', state: 'active' } as ThreadState['meta'],
      pendingUserMessages: [{ text: 'hello there', eventId: 'e1', created: IDLE_AT }],
    })]]);
    noteLocalComposeEdit('t1');
    upsertThread(map, summary({ state: 'composing', summary_version: LIVE, compose_text: 'hello th' }), false);
    expect(map.get('t1')!.meta.state).toBe('active');
    expect(getDraft('t1').text).toBe('');
  });

  it('applies state from a newer GET snapshot', () => {
    const map = new Map([['t1', liveThread({ meta: { id: 't1', state: 'composing' } as ThreadState['meta'] })]]);
    upsertThread(map, summary({ state: 'active', summary_version: LIVE + 1 }), false);
    expect(map.get('t1')!.meta.state).toBe('active');
  });

  it('refuses an older snapshot even when its last_activity ties the live one', () => {
    const map = new Map([['t1', liveThread()]]);
    upsertThread(map, summary({ status: 'running', summary_version: LIVE - 1, last_activity: IDLE_AT }), false);
    expect(map.get('t1')!.meta.status).toBe('idle');
  });

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
