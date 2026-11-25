import { describe, it, expect, beforeEach } from 'vitest';
import { promptAnimating, focusedThreadId, threadMap } from '../../../store/store';
import { activeExchanges } from '../../../store/store';
import { hasContentEvents, type ThreadState, type StoredEvent } from '../../../store/thread-events';
import { emptyReason } from '../ThreadView';

function makeThreadState(id: string): ThreadState {
  const events = new Map<number, StoredEvent>();
  events.set(1, { type: 'MessageReceived', text: 'hello', channel: 'chat', created: '2026-01-01T00:00:00Z' });
  events.set(2, { type: 'ResponseGenerated', text: 'hi back', created: '2026-01-01T00:00:01Z' });
  return {
    meta: { id, title: 'Test', channel: 'chat', initiator: 'user', saved: false, createdAt: '', updatedAt: '', summaryVersion: 0, status: 'idle', codingAgentChangeState: { kind: 'none' }, codingAgentIsExternalRepo: false, lastRevivedAt: '', messageCount: 1, section: 'archived', activeChildrenCount: 0, totalChildrenCount: 0, blockingDescendantCount: 0, attentionDescendantCount: 0, archivableSubThreadCount: 0, liveEventWaitCount: 0, state: 'active', latestTodoList: null, liveEventWaits: [] },
    events,
    streamingBuffer: '',
    eventsLoaded: true,
    eventsLoadFailed: false,
    lastDbSeq: 0,
    pendingUserMessages: [],
  };
}

describe('promptAnimating gate', () => {
  beforeEach(() => {
    promptAnimating.value = false;
    focusedThreadId.value = null;
    threadMap.value = new Map();
  });

  it('activeExchanges returns data when promptAnimating is false', () => {
    const id = 'thread-gate-1';
    focusedThreadId.value = id;
    threadMap.value = new Map([[id, makeThreadState(id)]]);

    expect(promptAnimating.value).toBe(false);
    expect(activeExchanges.value.length).toBeGreaterThan(0);
  });

  it('promptAnimating starts false (no gate on normal renders)', () => {
    expect(promptAnimating.value).toBe(false);
  });

  it('promptAnimating can be set and cleared', () => {
    promptAnimating.value = true;
    expect(promptAnimating.value).toBe(true);
    promptAnimating.value = false;
    expect(promptAnimating.value).toBe(false);
  });

  it('ThreadView gating logic: exchanges empty when animating, populated when not', () => {
    const id = 'thread-gate-2';
    focusedThreadId.value = id;
    const state = makeThreadState(id);
    threadMap.value = new Map([[id, state]]);

    const eventsLoaded = state.eventsLoaded;

    promptAnimating.value = true;
    const gatedExchanges = eventsLoaded && !promptAnimating.value ? activeExchanges.value : [];
    expect(gatedExchanges).toEqual([]);

    promptAnimating.value = false;
    const ungatedExchanges = eventsLoaded && !promptAnimating.value ? activeExchanges.value : [];
    expect(ungatedExchanges.length).toBeGreaterThan(0);
  });

  it('emptyReason returns animating during animation regardless of event state', () => {
    expect(emptyReason(true, false, false, false, 't1', false)).toEqual({ kind: 'animating' });
    expect(emptyReason(true, true, false, false, 't1', false)).toEqual({ kind: 'animating' });
    expect(emptyReason(true, true, false, true, 't1', false)).toEqual({ kind: 'animating' });
    expect(emptyReason(true, false, true, false, 't1', false)).toEqual({ kind: 'animating' });
  });

  it('emptyReason returns failed when load failed', () => {
    expect(emptyReason(false, false, true, false, 't1', false)).toEqual({ kind: 'failed', threadId: 't1' });
  });

  it('emptyReason returns corrupt only when CONTENT events exist but exchanges are empty', () => {
    expect(emptyReason(false, true, false, true, 't1', false)).toEqual({ kind: 'corrupt', threadId: 't1' });
  });

  it('emptyReason returns empty for genuinely empty thread', () => {
    expect(emptyReason(false, true, false, false, 't1', false)).toEqual({ kind: 'empty' });
  });

  it('emptyReason returns loading when events not yet loaded', () => {
    expect(emptyReason(false, false, false, false, 't1', false)).toEqual({ kind: 'loading', threadId: 't1' });
  });

  it('emptyReason returns disconnected when unloaded AND the engine is unreachable', () => {
    expect(emptyReason(false, false, false, false, 't1', true)).toEqual({ kind: 'disconnected', threadId: 't1' });
  });

  it('emptyReason: disconnected ONLY overrides loading — failed/corrupt/empty/animating keep precedence', () => {
    expect(emptyReason(true, false, false, false, 't1', true)).toEqual({ kind: 'animating' });
    expect(emptyReason(false, false, true, false, 't1', true)).toEqual({ kind: 'failed', threadId: 't1' });
    expect(emptyReason(false, true, false, true, 't1', true)).toEqual({ kind: 'corrupt', threadId: 't1' });
    expect(emptyReason(false, true, false, false, 't1', true)).toEqual({ kind: 'empty' });
  });

  it('error state must NOT flash during animation even when events exist (thread creation bug)', () => {
    const id = 'thread-error-flash';
    focusedThreadId.value = id;
    const events = new Map<number, StoredEvent>();
    events.set(1, { type: 'MessageReceived', text: 'hello', channel: 'chat', created: '2026-01-01T00:00:00Z' });

    const state: ThreadState = {
      meta: { id, title: '', channel: 'chat', initiator: 'user', saved: false, createdAt: '', updatedAt: '', summaryVersion: 0, status: 'idle', codingAgentChangeState: { kind: 'none' }, codingAgentIsExternalRepo: false, lastRevivedAt: '', messageCount: 1, section: 'archived', activeChildrenCount: 0, totalChildrenCount: 0, blockingDescendantCount: 0, attentionDescendantCount: 0, archivableSubThreadCount: 0, liveEventWaitCount: 0, state: 'active', latestTodoList: null, liveEventWaits: [] },
      events,
      streamingBuffer: '',
      eventsLoaded: true,
      eventsLoadFailed: false,
      lastDbSeq: 0,
      pendingUserMessages: [],
    };
    threadMap.value = new Map([[id, state]]);

    const reason = emptyReason(true, state.eventsLoaded, state.eventsLoadFailed, hasContentEvents(state.events), id, false);
    expect(reason.kind).toBe('animating');

    promptAnimating.value = false;
    const exchanges = activeExchanges.value;
    expect(exchanges.length).toBeGreaterThan(0);
  });

  it('promptAnimating stuck true blocks content permanently (reproduces mobile bug)', () => {
    const id = 'thread-stuck-1';
    focusedThreadId.value = id;
    const state = makeThreadState(id);
    threadMap.value = new Map([[id, state]]);

    promptAnimating.value = true;

    const eventsLoaded = state.eventsLoaded;
    expect(eventsLoaded).toBe(true);
    const exchanges = eventsLoaded && !promptAnimating.value ? activeExchanges.value : [];
    expect(exchanges).toEqual([]);

    promptAnimating.value = false;
    const afterTimeout = eventsLoaded && !promptAnimating.value ? activeExchanges.value : [];
    expect(afterTimeout.length).toBeGreaterThan(0);
  });
});
