/**
 * Tests for the optimistic "answering" flag and `isRenderedThreadIdle`.
 *
 * Bug: after the user answers a pending question, the agent's resume can land
 * its first events while the client's `meta.status` still reads
 * `waiting_for_user_answer` (the `running` aggregate hadn't arrived yet). That
 * status is quiescent, so the lifted `threadIdle` prop was true and the
 * answered question-divider flashed "Aborted ⚠" via the stale-detector in
 * exchange-render.ts — even though the response completed normally.
 *
 * Fix: `answerThreadQuestion` stamps the thread in `answeringThreadIds`, and
 * `isRenderedThreadIdle` (the source for `threadIdle`) treats the thread as
 * NON-idle while that flag — or a pending follow-up — is in flight, bridging
 * the answer→resume gap until the real status leaves `waiting_for_user_answer`.
 */
import { describe, it, expect, afterEach } from 'vitest';
import {
  isRenderedThreadIdle,
  answeringThreadIds,
  markThreadAnswering,
  clearThreadAnswering,
} from './store';
import type { ThreadState, ThreadStatus } from './thread-events';

function makeThread(id = 'thread-1', status: ThreadStatus = 'idle'): ThreadState {
  return {
    meta: {
      id,
      title: 'Test',
      channel: 'chat',
      initiator: 'user',
      saved: false,
      createdAt: '',
      updatedAt: '',
      status,
      messageCount: 0,
      section: 'archived',
      activeChildrenCount: 0,
      totalChildrenCount: 0,
      blockingDescendantCount: 0,
      attentionDescendantCount: 0,
      codingAgentProposed: false,
      codingAgentRequiresRestart: false,
      codingAgentIsExternalRepo: false,
      codingAgentApplying: false,
      codingAgentHasDiff: false,
      lastRevivedAt: '',
      state: 'active',
      latestTodoList: null,
    },
    events: new Map(),
    streamingBuffer: '',
    eventsLoaded: true,
    eventsLoadFailed: false,
    lastDbSeq: 0,
    pendingUserMessages: [],
  };
}

describe('isRenderedThreadIdle', () => {
  afterEach(() => {
    answeringThreadIds.value = new Set();
  });

  it('treats a quiescent thread as idle when nothing is in flight', () => {
    expect(isRenderedThreadIdle(makeThread('t1', 'idle'))).toBe(true);
    expect(isRenderedThreadIdle(makeThread('t1', 'waiting_for_user_answer'))).toBe(true);
  });

  it('treats a running thread as not idle', () => {
    expect(isRenderedThreadIdle(makeThread('t1', 'running'))).toBe(false);
  });

  it('treats an undefined thread as not idle (matches the prior ?.status fallback)', () => {
    expect(isRenderedThreadIdle(undefined)).toBe(false);
  });

  it('suppresses idle while a question answer is in flight (the bug)', () => {
    const thread = makeThread('t1', 'waiting_for_user_answer');
    expect(isRenderedThreadIdle(thread)).toBe(true);
    markThreadAnswering('t1');
    // The answer→resume gap: raw status still quiescent, but the optimistic
    // flag forces "not idle" so the divider can't flash "Aborted".
    expect(isRenderedThreadIdle(thread)).toBe(false);
  });

  it('falls back to raw status once the answering flag is cleared', () => {
    const thread = makeThread('t1', 'waiting_for_user_answer');
    markThreadAnswering('t1');
    expect(isRenderedThreadIdle(thread)).toBe(false);
    clearThreadAnswering('t1');
    expect(isRenderedThreadIdle(thread)).toBe(true);
  });

  it('suppresses idle while an un-ingested follow-up is pending', () => {
    const thread = makeThread('t1', 'idle');
    thread.pendingUserMessages.push({ text: 'hi', eventId: 'e1', created: new Date().toISOString() });
    expect(isRenderedThreadIdle(thread)).toBe(false);
  });

  it('answering flag is scoped per thread', () => {
    markThreadAnswering('t1');
    expect(isRenderedThreadIdle(makeThread('t1', 'waiting_for_user_answer'))).toBe(false);
    expect(isRenderedThreadIdle(makeThread('t2', 'waiting_for_user_answer'))).toBe(true);
  });
});

describe('markThreadAnswering / clearThreadAnswering', () => {
  afterEach(() => {
    answeringThreadIds.value = new Set();
  });

  it('add then remove toggles membership and is idempotent', () => {
    markThreadAnswering('t1');
    markThreadAnswering('t1'); // idempotent — no duplicate / no throw
    expect(answeringThreadIds.value.has('t1')).toBe(true);
    clearThreadAnswering('t1');
    expect(answeringThreadIds.value.has('t1')).toBe(false);
    clearThreadAnswering('t1'); // idempotent on a missing id
    expect(answeringThreadIds.value.has('t1')).toBe(false);
  });
});
