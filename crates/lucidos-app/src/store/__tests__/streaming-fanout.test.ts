/**
 * Contract: SSE event arrivals split fan-out into two signals.
 *
 *   - `threadMap` (wide) fires ONLY when a meta-shape field actually changed
 *     value. `attentionThreadCount`, `ThreadDrawer.ThreadList`, every
 *     subscribed `ChatExchange`, and every `PromptInput` effect read this
 *     signal — they must stop re-executing per CC streaming token.
 *
 *   - `threadEventsBump` (per-thread, in `store/threadActivity.ts`) fires
 *     for every event arrival in that thread. Focused-thread views
 *     (`activeExchanges`, `activeStreamingBuffer`, ThreadView's
 *     `computeExchanges` memo) subscribe to it for the focused id only.
 *
 * An iOS PWA used to lag whenever a long-running CC subprocess
 * streamed ~70 events/min — the wide `threadMap` fire cascaded global
 * re-renders. See `~/.claude/plans/generic-sparking-garden.md`.
 */
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { effect } from '@preact/signals';
import { threadMap, focusedThreadId, attentionThreadCount, activeExchanges } from '../store';
import { handleThreadEvent } from '../actions/thread-sync';
import { bumpThreadEvents } from '../threadActivity';
import {
  getThreadEventsBump,
  _resetThreadEventsBumpForTesting,
  flushThreadEventsBumpsNow,
} from '../threadActivity';
import type { ThreadState, ThreadAggregate } from '../thread-events';

// handleThreadEvent uses RAF for batched signal updates; tests step through
// it with setTimeout 0 like the sibling thread-sync tests.
vi.stubGlobal('requestAnimationFrame', (cb: FrameRequestCallback) => setTimeout(cb, 0));
vi.stubGlobal('cancelAnimationFrame', (id: number) => clearTimeout(id));

function makeThread(id: string, overrides: Partial<ThreadState['meta']> = {}): ThreadState {
  return {
    meta: {
      id,
      title: 'Test Thread',
      channel: 'chat',
      initiator: 'user',
      saved: false,
      createdAt: '2026-01-01T00:00:00Z',
      updatedAt: '2026-01-01T00:00:00Z',
      status: 'idle',
      messageCount: 1,
      section: 'archived',
      activeChildrenCount: 0,
      totalChildrenCount: 0,
      blockingDescendantCount: 0,
      attentionDescendantCount: 0,
      liveEventWaitCount: 0,
      codingAgentProposed: false,
      codingAgentRequiresRestart: false,
      codingAgentIsExternalRepo: false,
      codingAgentApplying: false,
      codingAgentHasDiff: false,
      lastRevivedAt: '',
      state: 'active',
      latestTodoList: null,
      liveEventWaits: [],
      ...overrides,
    },
    events: new Map(),
    streamingBuffer: '',
    eventsLoaded: true,
    eventsLoadFailed: false,
    lastDbSeq: 0,
    pendingUserMessages: [],
  };
}

function makeAggregate(id: string, overrides: Partial<ThreadAggregate> = {}): ThreadAggregate {
  return {
    threadId: id,
    title: 'Test Thread',
    channel: 'chat',
    initiator: 'user',
    createdAt: '2026-01-01T00:00:00Z',
    lastActivity: '2026-04-17T00:00:00Z',
    messageCount: 1,
    section: 'archived',
    status: 'idle',
    activeChildrenCount: 0,
    totalChildrenCount: 0,
    blockingDescendantCount: 0,
    attentionDescendantCount: 0,
    liveEventWaitCount: 0,
    codingAgentProposed: false,
    codingAgentRequiresRestart: false,
    codingAgentIsExternalRepo: false,
    codingAgentApplying: false,
    codingAgentHasDiff: false,
    isSaved: false,
    hasResponse: true,
    lastRevivedAt: null,
    parentThreadId: null,
    parentThreadTitle: null,
    state: 'active',
    ...overrides,
  };
}

describe('SSE streaming fan-out — split between threadMap and threadEventsBump', () => {
  beforeEach(() => {
    threadMap.value = new Map();
    focusedThreadId.value = null;
    _resetThreadEventsBumpForTesting();
  });

  it('CodingAgentTextStreamed bumps per-thread signal and does NOT change threadMap reference', () => {
    const id = 'thread-stream';
    const map = threadMap.value;
    map.set(id, makeThread(id, { channel: 'claude_code', status: 'running' }));
    // Don't reassign threadMap.value — we want to assert the SSE handler
    // doesn't either (the whole point of the patch).
    const mapBefore = threadMap.value;
    const bumpBefore = getThreadEventsBump(id);

    handleThreadEvent({
      thread_id: id,
      seq: 5,
      event: { type: 'CodingAgentTextStreamed', text: 'hello world' },
      created: '2026-04-17T00:00:01Z',
      aggregate: makeAggregate(id, { channel: 'claude_code', status: 'running', lastActivity: '2026-04-17T00:00:01Z' }),
    });
    flushThreadEventsBumpsNow();

    expect(threadMap.value).toBe(mapBefore);
    expect(getThreadEventsBump(id)).toBeGreaterThan(bumpBefore);
  });

  it('ThreadTitleGenerated bumps AND publishes a fresh threadMap reference', () => new Promise<void>((resolve) => {
    const id = 'thread-title';
    const map = threadMap.value;
    map.set(id, makeThread(id, { title: '...' }));
    const mapBefore = threadMap.value;
    const bumpBefore = getThreadEventsBump(id);

    handleThreadEvent({
      thread_id: id,
      seq: 7,
      event: { type: 'ThreadTitleGenerated', title: 'A real title' },
      created: '2026-04-17T00:00:02Z',
      aggregate: makeAggregate(id, { title: 'A real title', lastActivity: '2026-04-17T00:00:02Z' }),
    });
    flushThreadEventsBumpsNow();

    // threadMap flush is RAF-deferred via setTimeout in this test env.
    setTimeout(() => {
      expect(threadMap.value).not.toBe(mapBefore);
      expect(threadMap.value.get(id)?.meta.title).toBe('A real title');
      expect(getThreadEventsBump(id)).toBeGreaterThan(bumpBefore);
      resolve();
    }, 10);
  }));

  it('20 CodingAgentTextStreamed events on a non-focused thread do not re-execute attentionThreadCount', () => {
    const streamId = 'streaming-thread';
    const otherId = 'other-thread';
    const map = threadMap.value;
    map.set(streamId, makeThread(streamId, { channel: 'claude_code', status: 'running' }));
    map.set(otherId, makeThread(otherId));
    // Trigger one flush so the new threads are visible to the computed.
    threadMap.value = new Map(map);
    focusedThreadId.value = otherId;

    let runs = 0;
    const dispose = effect(() => {
      // Read the computed to subscribe; .value is the contract.
      void attentionThreadCount.value;
      runs += 1;
    });
    const baseline = runs;

    for (let i = 0; i < 20; i++) {
      handleThreadEvent({
        thread_id: streamId,
        seq: 100 + i,
        event: { type: 'CodingAgentTextStreamed', text: `token ${i}` },
        created: '2026-04-17T00:00:01Z',
        aggregate: makeAggregate(streamId, { channel: 'claude_code', status: 'running', lastActivity: '2026-04-17T00:00:01Z' }),
      });
    }
    flushThreadEventsBumpsNow();

    dispose();
    // The baseline `runs` is the initial fire. No streaming event should have
    // pushed `threadMap`, so the computed must not have re-executed.
    expect(runs).toBe(baseline);
  });

  it('activeExchanges recomputes on the FOCUSED thread bump and NOT on a different thread bump', () => {
    const focusedId = 'focused';
    const otherId = 'other';
    const map = threadMap.value;
    map.set(focusedId, makeThread(focusedId, { channel: 'claude_code', status: 'running' }));
    map.set(otherId, makeThread(otherId, { channel: 'claude_code', status: 'running' }));
    threadMap.value = new Map(map);
    focusedThreadId.value = focusedId;

    let runs = 0;
    const dispose = effect(() => {
      void activeExchanges.value;
      runs += 1;
    });
    const baseline = runs;

    // Burst on OTHER thread — focused subscriber must stay cold.
    for (let i = 0; i < 10; i++) {
      handleThreadEvent({
        thread_id: otherId,
        seq: 200 + i,
        event: { type: 'CodingAgentTextStreamed', text: `o${i}` },
        created: '2026-04-17T00:00:01Z',
        aggregate: makeAggregate(otherId, { channel: 'claude_code', status: 'running', lastActivity: '2026-04-17T00:00:01Z' }),
      });
    }
    flushThreadEventsBumpsNow();
    expect(runs).toBe(baseline);

    // Single event on FOCUSED thread — subscriber must wake.
    handleThreadEvent({
      thread_id: focusedId,
      seq: 300,
      event: { type: 'CodingAgentTextStreamed', text: 'mine' },
      created: '2026-04-17T00:00:02Z',
      aggregate: makeAggregate(focusedId, { channel: 'claude_code', status: 'running', lastActivity: '2026-04-17T00:00:02Z' }),
    });
    flushThreadEventsBumpsNow();
    expect(runs).toBeGreaterThan(baseline);

    dispose();
  });

  it('repeated aggregates whose fields match exactly do not flip threadMap', () => new Promise<void>((resolve) => {
    const id = 'stable';
    const map = threadMap.value;
    map.set(id, makeThread(id, { status: 'running', section: 'inbox' }));
    threadMap.value = new Map(map);
    const mapBefore = threadMap.value;

    // Two streaming events with identical aggregates — meta doesn't move.
    for (let i = 0; i < 2; i++) {
      handleThreadEvent({
        thread_id: id,
        seq: 400 + i,
        event: { type: 'CodingAgentTextStreamed', text: `t${i}` },
        created: '2026-04-17T00:00:03Z',
        aggregate: makeAggregate(id, { status: 'running', section: 'inbox', lastActivity: '2026-04-17T00:00:03Z' }),
      });
    }
    flushThreadEventsBumpsNow();

    setTimeout(() => {
      expect(threadMap.value).toBe(mapBefore);
      resolve();
    }, 10);
  }));

  it('optimistic pending-message insertion + per-thread bump wakes activeExchanges', () => {
    // Regression test for the addPendingMessage / unreachable-engine paths in
    // chat.ts. Those code paths mutate `thread.pendingUserMessages` (or call
    // `handleEvent` to insert a synthetic ResponseFailed) and reassign
    // `threadMap.value = new Map(...)`. Without a paired `bumpThreadEvents`,
    // `activeExchanges` (which subscribes only to focusedThreadId + the
    // per-thread bump, then reads `threadMap.peek()`) keeps its cached value
    // and the synthetic exchange does not render. The test models the same
    // mutation pattern directly so any future caller forgetting the pairing
    // trips the assertion.
    const id = 'pending-thread';
    const map = threadMap.value;
    map.set(id, makeThread(id));
    threadMap.value = new Map(map);
    focusedThreadId.value = id;

    let runs = 0;
    let lastLength = 0;
    const dispose = effect(() => {
      const exchanges = activeExchanges.value;
      lastLength = exchanges.length;
      runs += 1;
    });
    const baseline = runs;
    expect(lastLength).toBe(0);

    // Same mutation pattern as `addPendingMessage`: push to pendingUserMessages,
    // reassign threadMap, fire the per-thread bump.
    const thread = threadMap.value.get(id)!;
    thread.pendingUserMessages.push({
      text: 'hello',
      eventId: 'evt-1',
      created: '2026-04-17T00:00:04Z',
    });
    threadMap.value = new Map(threadMap.value);
    bumpThreadEvents(id);
    flushThreadEventsBumpsNow();

    expect(runs).toBeGreaterThan(baseline);
    expect(lastLength).toBe(1);

    dispose();
  });
});
