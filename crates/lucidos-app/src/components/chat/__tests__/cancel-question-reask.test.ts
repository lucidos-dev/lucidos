import { describe, it, expect } from 'vitest';
import {
  findLatestPendingQuestion,
  shouldClearCanceling,
} from '../prompt-input-helpers';
import type { ThreadEvent, ThreadMeta, ThreadState } from '../../../store/thread-events';

function buildThreadState(
  events: ThreadEvent[],
  status: ThreadMeta['status'] = 'waiting_for_user_answer',
): ThreadState {
  const map = new Map<number, ThreadEvent>();
  events.forEach((ev, i) => map.set(i + 1, ev));
  const meta: ThreadMeta = {
    id: 'thread-1',
    title: 'Test',
    channel: 'chat',
    initiator: 'user',
    saved: false,
    createdAt: '2026-01-01T00:00:00Z',
    updatedAt: '2026-01-01T00:00:00Z',
    status,
    summaryVersion: 0,
    codingAgentChangeState: { kind: 'none' },
    codingAgentIsExternalRepo: false,
    lastRevivedAt: '',
    messageCount: 1,
    section: 'archived',
    activeChildrenCount: 0,
    totalChildrenCount: 0,
    blockingDescendantCount: 0,
    attentionDescendantCount: 0,
    archivableSubThreadCount: 0,
    state: 'active',
    latestTodoList: null,
    liveEventWaitCount: 0,
    liveEventWaits: [],
  };
  return {
    meta,
    events: map,
    streamingBuffer: '',
    eventsLoaded: true,
    eventsLoadFailed: false,
    lastDbSeq: events.length,
    pendingUserMessages: [],
  };
}

const ask = (tool_use_id: string, multi_select = false): ThreadEvent =>
  ({
    type: 'UserQuestionAsked',
    tool_use_id,
    cc_session_id: '',
    question: 'Want me to set up Slack access?',
    options: [{ id: 'opt-0', label: 'Bot token' }],
    multi_select,
  }) as ThreadEvent;

const answer = (tool_use_id: string): ThreadEvent =>
  ({
    type: 'UserQuestionAnswered',
    tool_use_id,
    answer: { kind: 'Canceled' },
  }) as ThreadEvent;

describe('findLatestPendingQuestion', () => {
  it('returns the latest unanswered question (single OR multi)', () => {
    const single = buildThreadState([
      { type: 'MessageReceived', text: 'go', channel: 'chat' } as ThreadEvent,
      ask('tu_q1', false),
    ]);
    expect(findLatestPendingQuestion(single)).toEqual({ toolUseId: 'tu_q1', multiSelect: false });

    const multi = buildThreadState([
      { type: 'MessageReceived', text: 'go', channel: 'chat' } as ThreadEvent,
      ask('tu_q1', true),
    ]);
    expect(findLatestPendingQuestion(multi)).toEqual({ toolUseId: 'tu_q1', multiSelect: true });
  });

  it('returns null when the latest question is already answered', () => {
    const thread = buildThreadState([
      { type: 'MessageReceived', text: 'go', channel: 'chat' } as ThreadEvent,
      ask('tu_q1'),
      answer('tu_q1'),
    ]);
    expect(findLatestPendingQuestion(thread)).toBeNull();
  });

  it('after a re-ask, returns the NEW pending question, not the canceled one', () => {
    const thread = buildThreadState([
      { type: 'MessageReceived', text: 'go', channel: 'chat' } as ThreadEvent,
      ask('tu_q1'),
      answer('tu_q1'),
      ask('tu_q2'),
    ]);
    expect(findLatestPendingQuestion(thread)).toEqual({ toolUseId: 'tu_q2', multiSelect: false });
  });

  it('returns null for an undefined thread', () => {
    expect(findLatestPendingQuestion(undefined)).toBeNull();
  });
});

describe('shouldClearCanceling', () => {
  it('clears once the thread leaves every mid-turn status (turn ended)', () => {
    expect(shouldClearCanceling('idle', undefined, undefined)).toBe(true);
    expect(shouldClearCanceling('waiting', undefined, undefined)).toBe(true);
  });

  it('keeps the flag while still waiting on the SAME question that was canceled', () => {
    expect(shouldClearCanceling('waiting_for_user_answer', 'tu_q1', 'tu_q1')).toBe(false);
  });

  it('clears when the canceled question is replaced by a re-asked one', () => {
    expect(shouldClearCanceling('waiting_for_user_answer', 'tu_q1', 'tu_q2')).toBe(true);
  });

  it('clears when the canceled question resolved and nothing is pending yet (agent resumed running)', () => {
    expect(shouldClearCanceling('running', 'tu_q1', undefined)).toBe(true);
  });

  it('keeps the flag for a running-turn cancel (no question targeted) while still running', () => {
    expect(shouldClearCanceling('running', undefined, undefined)).toBe(false);
  });

  it('releases a running-turn cancel the moment the turn pauses on a NEW card it never targeted', () => {
    expect(shouldClearCanceling('waiting_for_user_answer', undefined, undefined, false)).toBe(true);
  });

  it('keeps a card cancel (permission/question) bridged through waiting_for_user_answer', () => {
    expect(shouldClearCanceling('waiting_for_user_answer', undefined, undefined, true)).toBe(false);
  });

  it('releases a card cancel once the turn terminates (idle/waiting)', () => {
    expect(shouldClearCanceling('idle', undefined, undefined, true)).toBe(true);
    expect(shouldClearCanceling('waiting', undefined, undefined, true)).toBe(true);
  });
});

describe('integration: cancel a question, agent re-asks', () => {
  it('releases the optimistic canceling flag for the re-asked question', () => {
    const thread = buildThreadState([
      { type: 'MessageReceived', text: 'go', channel: 'chat' } as ThreadEvent,
      ask('tu_q1'),
      answer('tu_q1'),
      ask('tu_q2'),
    ]);
    const canceledQuestionId = 'tu_q1';
    const latestPending = findLatestPendingQuestion(thread)?.toolUseId;
    expect(latestPending).toBe('tu_q2');
    expect(
      shouldClearCanceling(thread.meta.status, canceledQuestionId, latestPending),
    ).toBe(true);
  });
});
