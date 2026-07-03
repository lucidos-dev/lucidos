// Shared scaffolding for the thread-wiring tests, split across
// thread-wiring-*.test.ts.
import { type ThreadState } from '../thread-events';

export const TS = '2026-04-17T00:00:00Z';

export function makeThread(overrides: Partial<ThreadState> = {}): ThreadState {
  return {
    meta: {
      id: 'thread-1',
      title: '...',
      channel: 'chat',
      initiator: 'user',
      saved: false,
      createdAt: '',
      updatedAt: '',
      status: 'idle',
      codingAgentProposed: false,
      codingAgentRequiresRestart: false,
      codingAgentIsExternalRepo: false,
      codingAgentApplying: false,
      codingAgentHasDiff: false,
      lastRevivedAt: '',
      messageCount: 0,
      section: 'archived',
      activeChildrenCount: 0,
      totalChildrenCount: 0,
      blockingDescendantCount: 0, attentionDescendantCount: 0,
      state: 'active',
      latestTodoList: null,
    },
    events: new Map(),
    streamingBuffer: '',
    eventsLoaded: false,
    eventsLoadFailed: false,
    lastDbSeq: 0,
    pendingUserMessages: [],
    ...overrides,
  };
}
