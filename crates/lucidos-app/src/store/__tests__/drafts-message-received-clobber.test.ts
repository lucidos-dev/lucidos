/**
 * A thread event never touches the draft (ADR 0450). Regression: a typed,
 * unsent follow-up on an ACTIVE thread was blanked by a peer's
 * `MessageReceived` echo while the user was off the thread.
 *
 * A send consumes the compose slot when the engine accepts it, and the engine
 * reports what that left as a *compose report*. So the only thing that clears
 * a draft from outside is a report at a newer revision, with no local edit
 * pending.
 */
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { threadMap, focusedThreadId } from '../store';
import { handleGlobalEvent, handleThreadEvent } from '../actions/thread-sync';
import { _resetComposeSyncForTesting } from '../composeSync';
import { getDraft, setDraft, _resetComposeDraftsForTesting } from '../composeDrafts';
import type { ThreadState } from '../thread-events';

// handleThreadEvent batches its threadMap flush via requestAnimationFrame; the
// compose-clear side effect itself is synchronous (clearDraft writes the signal
// immediately), so the draft assertions need no rAF wait.
vi.stubGlobal('requestAnimationFrame', (cb: FrameRequestCallback) => setTimeout(cb, 0));
vi.stubGlobal('cancelAnimationFrame', (id: number) => clearTimeout(id));

function makeActiveThread(id: string): ThreadState {
  return {
    meta: {
      id,
      title: 'T',
      channel: 'chat',
      initiator: 'user',
      saved: false,
      createdAt: '2026-01-01T00:00:00Z',
      updatedAt: '2026-01-01T00:00:00Z',
      status: 'idle',
      summaryVersion: 0,
      messageCount: 1,
      section: 'inbox',
      activeChildrenCount: 0,
      totalChildrenCount: 0,
      blockingDescendantCount: 0,
      attentionDescendantCount: 0,
      codingAgentChangeState: { kind: 'none' },
      codingAgentIsExternalRepo: false,
      lastRevivedAt: '',
      state: 'active',
      latestTodoList: null,
      liveEventWaitCount: 0,
      liveEventWaits: [],
    },
    events: new Map(),
    streamingBuffer: '',
    eventsLoaded: true,
    eventsLoadFailed: false,
    lastDbSeq: 0,
    pendingUserMessages: [],
  };
}

describe('a MessageReceived echo leaves the draft alone', () => {
  beforeEach(() => {
    threadMap.value = new Map();
    focusedThreadId.value = null;
    _resetComposeDraftsForTesting();
    _resetComposeSyncForTesting();
  });

  it('keeps a typed follow-up draft when a peer MessageReceived for the thread is processed', () => {
    const T = 'thread-T';
    threadMap.value = new Map([[T, makeActiveThread(T)]]);
    setDraft(T, { text: 'thread draft text', image_hashes: [], mode: null });

    handleThreadEvent({
      thread_id: T,
      seq: 1,
      event: { type: 'MessageReceived', text: 'an earlier message', device_id: 'peer-device' },
      created: '2026-06-28T00:00:00Z',
    });

    expect(getDraft(T).text).toBe('thread draft text');
  });

  it('clears a synced draft once the engine reports the slot it consumed', () => {
    const T = 'thread-T2';
    threadMap.value = new Map([[T, makeActiveThread(T)]]);
    handleGlobalEvent('ThreadComposeChanged', { id: T, summary_version: 4, compose_epoch: 0, text: 'server draft', image_hashes: [] });
    expect(getDraft(T).text).toBe('server draft');

    handleThreadEvent({
      thread_id: T,
      seq: 1,
      event: { type: 'MessageReceived', text: 'server draft', device_id: 'peer-device' },
      created: '2026-06-28T00:00:00Z',
    });
    expect(getDraft(T).text, 'the event alone says nothing about the draft').toBe('server draft');

    handleGlobalEvent('ThreadComposeChanged', { id: T, summary_version: 5, compose_epoch: 1, text: '', image_hashes: [] });
    expect(getDraft(T).text).toBe('');
  });
});
