import { describe, it, expect, beforeEach, vi } from 'vitest';
import { computeMorphMode, shouldClearSubmitting } from '../prompt-input-helpers';
import { getWaitingState } from '../WaitingBanner';
import {
  threadMap,
  focusedThreadId,
  applyNowRequests,
  applyingChangeIds,
  discardingCCThreadIds,
  cancelingThreadIds,
  changes,
} from '../../../store/store';
import type { EventWaitSummary, ThreadState, ThreadStatus } from '../../../store/thread-events';
import { _resetRowIntentsForTesting } from '../../../store/rowIntents';

vi.mock('../../../store/actions/threadActions', () => ({
  resolveThreadActions: () => [
    { kind: 'archive', category: 'close', label: 'Archive', invoke: () => {} },
  ],
  threadHasIncompleteChange: () => false,
}));
vi.mock('../../../store/actions/repositories', () => ({
  viewChangeDiff: vi.fn(),
  viewThreadCcDiff: vi.fn(),
}));


const LIVE_WAIT: EventWaitSummary = {
  wait_id: 'w-1',
  on: [{ event_type: 'ChangeProposed' }],
  reason: 'waiting for the release build to finish',
  expires_at: new Date(Date.now() + 3_600_000).toISOString(),
};

function makeThread(
  id: string,
  status: ThreadStatus,
  overrides: Partial<ThreadState['meta']> = {},
): ThreadState {
  return {
    meta: {
      id,
      title: 'test',
      channel: 'chat',
      initiator: 'user',
      saved: false,
      createdAt: '',
      updatedAt: '',
      status,
      summaryVersion: 0,
      messageCount: 1,
      section: 'inbox',
      activeChildrenCount: 0,
      totalChildrenCount: 0,
      blockingDescendantCount: 0,
      attentionDescendantCount: 0,
      archivableSubThreadCount: 0,
      codingAgentChangeState: { kind: 'none' },
      codingAgentIsExternalRepo: false,
      lastRevivedAt: '',
      state: 'active',
      latestTodoList: null,
      liveEventWaitCount: 0,
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

function focus(thread: ThreadState): void {
  threadMap.value = new Map([[thread.meta.id, thread]]);
  focusedThreadId.value = thread.meta.id;
}

function morphMode(submitting: boolean): string {
  const waitingState = getWaitingState();
  const focused = focusedThreadId.value;
  const cancelTargetId =
    waitingState?.type === 'canceling' ? waitingState.threadId
    : (focused && submitting) ? focused
    : null;
  return computeMorphMode({
    hasContent: false,
    cancelTargetId,
    isCanceling: false,
    hasBannerOrSectionButtons:
      !!waitingState && waitingState.type !== 'canceling',
  });
}

beforeEach(() => {
  threadMap.value = new Map();
  focusedThreadId.value = null;
  _resetRowIntentsForTesting();
  applyNowRequests.value = new Map();
  applyingChangeIds.value = new Set();
  discardingCCThreadIds.value = new Set();
  cancelingThreadIds.value = new Set();
  changes.value = { status: 'loaded', data: [] };
});

describe('the Stop control appears only while a turn is in flight', () => {
  it('draws no Stop on an idle thread that is holding live subscriptions', () => {
    focus(makeThread('t-idle', 'idle', {
      liveEventWaitCount: 1,
      liveEventWaits: [LIVE_WAIT],
    }));
    expect(morphMode(false)).not.toBe('cancel');
  });

  it('draws no Stop on a thread whose turn failed', () => {
    focus(makeThread('t-failed', 'failed'));
    expect(shouldClearSubmitting('failed', false)).toBe(true);
    expect(morphMode(false)).not.toBe('cancel');
  });

  it('DOES draw Stop while a turn is streaming', () => {
    focus(makeThread('t-running', 'running', {
      liveEventWaitCount: 1,
      liveEventWaits: [LIVE_WAIT],
    }));
    expect(morphMode(false)).toBe('cancel');
  });

  it('DOES draw Stop while a question card is pending', () => {
    focus(makeThread('t-question', 'waiting_for_user_answer'));
    expect(morphMode(false)).toBe('cancel');
  });

  it('DOES draw Stop across the click to SSE gap, before the status catches up', () => {
    focus(makeThread('t-sending', 'idle'));
    expect(morphMode(true)).toBe('cancel');
  });
});

describe('shouldClearSubmitting', () => {
  it('releases once the real status takes over', () => {
    expect(shouldClearSubmitting('running', false)).toBe(true);
    expect(shouldClearSubmitting('waiting_for_user_answer', false)).toBe(true);
  });

  it('releases on a settled status, so the bridge cannot outlive the send', () => {
    for (const status of ['idle', 'failed', 'waiting', 'paused'] as ThreadStatus[]) {
      expect(shouldClearSubmitting(status, false)).toBe(true);
    }
  });

  it('holds while a send is queued behind an image upload', () => {
    expect(shouldClearSubmitting('idle', true)).toBe(false);
  });

  it('still releases on a queued upload once the turn is genuinely running', () => {
    expect(shouldClearSubmitting('running', true)).toBe(true);
  });
});
