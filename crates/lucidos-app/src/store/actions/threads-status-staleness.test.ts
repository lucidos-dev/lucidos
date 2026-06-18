/**
 * Regression: a completed thread's status dot stuck on "running" until reload.
 *
 * Root cause: both snapshot-application paths overwrote `meta.status` from a
 * backend snapshot with NO staleness guard, while every sibling field is
 * guarded. A resync GET (`loadAllThreads` → `upsertThread`, or
 * `refreshThreadEvents` → `applyEventRows`) that FIRED while the thread was
 * `running` but whose response LANDED after live SSE had already applied the
 * terminal `ResponseGenerated` (status='idle') clobbered idle → running. The
 * DB projection was idle, so a reload fetched the correct state — exactly the
 * "gone when I reloaded" symptom.
 *
 * Fix: `last_activity` (list GET), `currentAggregate.lastActivity` (per-thread
 * snapshot) and `meta.updatedAt` are all the SAME monotonic
 * `thread_summaries.last_activity` column read at different times. So
 * `snapshot.last_activity < meta.updatedAt` is an exact causal-staleness test:
 * the snapshot was captured before a live event we've already applied. Both
 * paths now skip the stale status overwrite.
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
import type { ThreadAggregate, ThreadState } from '../thread-events';
import type { ThreadSummary } from '../../api/threads';
import { focusedThreadId, threadMap } from '../store';
import { upsertThread, refreshThreadEvents } from './thread-loading';

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
    activeChildrenCount: 0,
    totalChildrenCount: 0,
    blockingDescendantCount: 0,
    attentionDescendantCount: 0,
    codingAgentHasDiff: false,
    codingAgentProposed: false,
    codingAgentRequiresRestart: false,
    codingAgentIsExternalRepo: false,
    codingAgentApplying: false,
    isSaved: false,
    hasResponse: true,
    lastRevivedAt: null,
    parentThreadId: null,
    parentThreadTitle: null,
    state: 'active',
    ...overrides,
  };
}

beforeEach(() => {
  threadMap.value = new Map();
  focusedThreadId.value = null;
});

describe('upsertThread — status staleness guard (loadAllThreads path)', () => {
  it('does not regress a live idle to running when the GET snapshot is stale', () => {
    const map = new Map<string, ThreadState>();
    // Live state: the terminal ResponseGenerated already applied idle and
    // advanced updatedAt to the idle event's last_activity.
    map.set('t1', makeThreadState('t1', { meta: { id: 't1', status: 'idle', updatedAt: IDLE_AT } }));

    // A loadAllThreads GET fired WHILE running (older last_activity), landing
    // after the live idle was applied.
    upsertThread(map, summary({ status: 'running', last_activity: RUNNING_AT }), false);

    expect(map.get('t1')!.meta.status).toBe('idle');
  });

  it('applies status from a fresh GET snapshot (last_activity >= live updatedAt)', () => {
    const map = new Map<string, ThreadState>();
    map.set('t1', makeThreadState('t1', { meta: { id: 't1', status: 'idle', updatedAt: RUNNING_AT } }));

    // A genuinely newer snapshot: the user sent a follow-up, status is running.
    upsertThread(map, summary({ status: 'running', last_activity: IDLE_AT }), false);

    expect(map.get('t1')!.meta.status).toBe('running');
  });
});

describe('refreshThreadEvents — status staleness guard (applyEventRows path)', () => {
  it('does not regress a live idle to running when the currentAggregate is stale', async () => {
    const map = new Map<string, ThreadState>();
    map.set('t1', makeThreadState('t1', {
      meta: { id: 't1', status: 'idle', updatedAt: IDLE_AT },
      eventsLoaded: true,
      lastDbSeq: 5,
    }));
    threadMap.value = map;

    const { fetchThreadEvents } = await import('../../api/threads');
    // Snapshot fetched before idle committed: no new events, status='running',
    // lastActivity older than what live SSE already applied.
    (fetchThreadEvents as any).mockResolvedValue({
      events: [],
      currentAggregate: aggregate({ status: 'running', lastActivity: RUNNING_AT }),
    });

    await refreshThreadEvents('t1');

    expect(threadMap.value.get('t1')!.meta.status).toBe('idle');
  });

  it('applies a fresh currentAggregate that legitimately advances status', async () => {
    const map = new Map<string, ThreadState>();
    map.set('t1', makeThreadState('t1', {
      meta: { id: 't1', status: 'idle', updatedAt: RUNNING_AT },
      eventsLoaded: true,
      lastDbSeq: 5,
    }));
    threadMap.value = map;

    const { fetchThreadEvents } = await import('../../api/threads');
    (fetchThreadEvents as any).mockResolvedValue({
      events: [],
      currentAggregate: aggregate({ status: 'running', lastActivity: IDLE_AT }),
    });

    await refreshThreadEvents('t1');

    expect(threadMap.value.get('t1')!.meta.status).toBe('running');
  });
});
