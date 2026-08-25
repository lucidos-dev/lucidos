import { describe, it, expect, beforeEach, vi } from 'vitest';
import { computeMorphMode, shouldClearSubmitting } from '../prompt-input-helpers';
import { getWaitingState } from '../WaitingBanner';
import {
  threadMap,
  focusedThreadId,
  archivingThreadIds,
  applyingNowThreadIds,
  applyingChangeIds,
  discardingCCThreadIds,
  cancelingThreadIds,
  changes,
} from '../../../store/store';
import type { EventWaitSummary, ThreadState, ThreadStatus } from '../../../store/thread-events';

// `getWaitingState` reaches `resolveThreadActions`, whose real module pulls the
// whole action graph (archive/apply/discard handlers). Only the close-set
// filter matters here, and every thread below is an ordinary idle chat thread
// whose close set is Archive.
vi.mock('../../../store/actions/threadActions', () => ({
  resolveThreadActions: () => [
    { kind: 'archive', category: 'close', label: 'Archive', invoke: () => {} },
  ],
}));
vi.mock('../../../store/actions/repositories', () => ({
  viewChangeDiff: vi.fn(),
  viewThreadCcDiff: vi.fn(),
}));

/**
 * The Stop control's visibility, pinned.
 *
 * **Stop is offered only while the focused thread has a turn in flight.** It is
 * one button, the prompt row's Send/Stop morph, and two inputs decide it:
 * `getWaitingState`'s mid-turn branch (the real status) and the optimistic
 * `submittingThreadIds` bridge (the click → SSE gap). Both are covered here,
 * because the rule is only as strong as its weaker half and the bridge is the
 * half that had no expiry.
 *
 * Since a thread-level Stop stopped ending subscriptions, a Stop offered with
 * no turn behind it does *nothing at all* when pressed, which is the strongest
 * possible reason not to draw one.
 */

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
      messageCount: 1,
      section: 'inbox',
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

/** The morph mode the prompt row would render, given the live signals plus
 *  whether the optimistic submitting flag is set. Mirrors PromptInput's own
 *  `cancelTargetId` / `computeMorphMode` wiring for a composer with no typed
 *  text, which is the only state where a Stop can be drawn at all. */
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
  archivingThreadIds.value = new Set();
  applyingNowThreadIds.value = new Map();
  applyingChangeIds.value = new Set();
  discardingCCThreadIds.value = new Set();
  cancelingThreadIds.value = new Set();
  changes.value = { status: 'loaded', data: [] };
});

describe('the Stop control appears only while a turn is in flight', () => {
  it('draws no Stop on an idle thread that is holding live subscriptions', () => {
    // The reported shape. The thread is asleep on purpose: it reads as Waiting
    // in the drawer and its waiting indicator lists what it watches. None
    // of that is a turn, and Stop no longer ends a subscription, so a Stop here
    // would be a red button that does nothing.
    focus(makeThread('t-idle', 'idle', {
      liveEventWaitCount: 1,
      liveEventWaits: [LIVE_WAIT],
    }));
    expect(morphMode(false)).not.toBe('cancel');
  });

  it('draws no Stop on a thread whose turn failed', () => {
    // The bridge leak: a send that reaches a terminal status without the client
    // ever observing `running` used to leave the optimistic flag set for the
    // life of the page, so the Stop stayed on an idle thread until reload.
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
    // A question-parked turn is a live turn, paused on the user. Stop is what
    // stamps the card Canceled, so hiding it here would strand the thread.
    focus(makeThread('t-question', 'waiting_for_user_answer'));
    expect(morphMode(false)).toBe('cancel');
  });

  it('DOES draw Stop across the click to SSE gap, before the status catches up', () => {
    // `sendMessage` inserts its optimistic pending row synchronously, so the
    // effective status is already `running` by the first render after Send.
    // The flag is the belt: assert the button is drawn either way.
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
    // No turn is running, but a real send is waiting on the hash and Stop is
    // what drops it, so the button must stay.
    expect(shouldClearSubmitting('idle', true)).toBe(false);
  });

  it('still releases on a queued upload once the turn is genuinely running', () => {
    expect(shouldClearSubmitting('running', true)).toBe(true);
  });
});
