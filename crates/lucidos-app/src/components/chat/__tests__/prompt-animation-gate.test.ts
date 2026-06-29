/**
 * Tests that promptAnimating gates ThreadView content during
 * the CreateThreadView → ThreadView transition animation.
 */
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
    meta: { id, title: 'Test', channel: 'chat', initiator: 'user', saved: false, createdAt: '', updatedAt: '', status: 'idle', codingAgentProposed: false, codingAgentRequiresRestart: false, codingAgentIsExternalRepo: false, codingAgentApplying: false, codingAgentHasDiff: false, lastRevivedAt: '', messageCount: 1, section: 'archived', activeChildrenCount: 0, totalChildrenCount: 0, blockingDescendantCount: 0, attentionDescendantCount: 0, state: 'active', latestTodoList: null },
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

    // Not animating — exchanges should be available
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

    // Simulate what ThreadView does:
    // const exchanges = eventsLoaded && !animating ? activeExchanges.value : [];
    const eventsLoaded = state.eventsLoaded;

    promptAnimating.value = true;
    const gatedExchanges = eventsLoaded && !promptAnimating.value ? activeExchanges.value : [];
    expect(gatedExchanges).toEqual([]);

    promptAnimating.value = false;
    const ungatedExchanges = eventsLoaded && !promptAnimating.value ? activeExchanges.value : [];
    expect(ungatedExchanges.length).toBeGreaterThan(0);
  });

  it('emptyReason returns animating during animation regardless of event state', () => {
    // During the FLIP animation, events may arrive via SSE (hasContent=true) and
    // eventsLoaded=true, but exchanges are gated to []. The 'animating' variant
    // renders nothing — it suppresses the error/empty flash (the union makes the
    // error state impossible here) AND keeps the loading skeleton from showing on
    // a brand-new thread's first prompt send (the content is the just-sent
    // message, not a DB fetch). Distinct from 'loading' (a real DB fetch).
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
    // The reported bug: a cold boot into an unreachable engine left the thread in
    // 'loading' forever (→ "Tap to reload"). When disconnected, an unloaded thread
    // is now honestly 'disconnected'.
    expect(emptyReason(false, false, false, false, 't1', true)).toEqual({ kind: 'disconnected', threadId: 't1' });
  });

  it('emptyReason: disconnected ONLY overrides loading — failed/corrupt/empty/animating keep precedence', () => {
    // A load that actually failed/completed is already honest; disconnection must
    // not mask it. Animation still wins outright (suppresses any flash).
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
      meta: { id, title: '', channel: 'chat', initiator: 'user', saved: false, createdAt: '', updatedAt: '', status: 'idle', codingAgentProposed: false, codingAgentRequiresRestart: false, codingAgentIsExternalRepo: false, codingAgentApplying: false, codingAgentHasDiff: false, lastRevivedAt: '', messageCount: 1, section: 'archived', activeChildrenCount: 0, totalChildrenCount: 0, blockingDescendantCount: 0, attentionDescendantCount: 0, state: 'active', latestTodoList: null },
      events,
      streamingBuffer: '',
      eventsLoaded: true,
      eventsLoadFailed: false,
      lastDbSeq: 0,
      pendingUserMessages: [],
    };
    threadMap.value = new Map([[id, state]]);

    // During animation: emptyReason forces 'animating' (never error), which
    // renders nothing — no skeleton, no error flash.
    const reason = emptyReason(true, state.eventsLoaded, state.eventsLoadFailed, hasContentEvents(state.events), id, false);
    expect(reason.kind).toBe('animating');

    // After animation: events compute into exchanges normally — no empty state
    promptAnimating.value = false;
    const exchanges = activeExchanges.value;
    expect(exchanges.length).toBeGreaterThan(0);
  });

  it('promptAnimating stuck true blocks content permanently (reproduces mobile bug)', () => {
    // Bug: On mobile, FLIP animation sets promptAnimating=true but transitionend
    // never fires (element off-screen during pane swipe). Content never renders.
    const id = 'thread-stuck-1';
    focusedThreadId.value = id;
    const state = makeThreadState(id);
    threadMap.value = new Map([[id, state]]);

    // Simulate stuck FLIP animation — promptAnimating stays true
    promptAnimating.value = true;

    // Even though events are loaded, content is gated
    const eventsLoaded = state.eventsLoaded;
    expect(eventsLoaded).toBe(true);
    const exchanges = eventsLoaded && !promptAnimating.value ? activeExchanges.value : [];
    expect(exchanges).toEqual([]); // Bug: content hidden

    // After safety timeout clears the flag, content should render
    promptAnimating.value = false;
    const afterTimeout = eventsLoaded && !promptAnimating.value ? activeExchanges.value : [];
    expect(afterTimeout.length).toBeGreaterThan(0); // Fix: content visible
  });
});
