import { describe, it, expect } from 'vitest';
import { applyAggregateToMeta, makeOptimisticThreadState, reviewTier, UNVERSIONED, type ThreadAggregate } from '../thread-meta';

const T = 't1';
const AT = '2026-09-29T12:00:00.000Z';
const LATER = '2026-09-29T12:05:00.000Z';

function aggregate(summaryVersion: number, over: Partial<ThreadAggregate> = {}): ThreadAggregate {
  return {
    threadId: T, title: 'Fix the bug', channel: 'claude_code', initiator: 'user',
    createdAt: AT, lastActivity: AT, messageCount: 1, section: 'inbox', status: 'idle', summaryVersion,
    activeChildrenCount: 0, totalChildrenCount: 0, blockingDescendantCount: 0,
    attentionDescendantCount: 0, archivableSubThreadCount: 0, liveEventWaitCount: 0, codingAgentChangeState: { kind: 'none' },
    codingAgentIsExternalRepo: false, isSaved: false, hasResponse: true, lastRevivedAt: null,
    parentThreadId: null, parentThreadTitle: null, state: 'active',
    ...over,
  };
}

function freshThread() {
  return makeOptimisticThreadState({
    id: T, title: 'Fix the bug', channel: 'claude_code', initiator: 'user',
    eventsLoaded: true, timestamp: AT, status: 'idle',
  });
}

describe('the aggregate overlay reports only shape changes', () => {
  it('a message-count and activity tick overlays the meta but reports no change', () => {
    const thread = freshThread();
    expect(thread.meta.summaryVersion).toBe(UNVERSIONED);
    applyAggregateToMeta(thread.meta, aggregate(1));
    expect(applyAggregateToMeta(thread.meta, aggregate(2, { messageCount: 5, lastActivity: LATER }))).toBe(false);
    expect(thread.meta.messageCount).toBe(5);
    expect(thread.meta.updatedAt).toBe(LATER);
  });

  it('a last-agent-action change overlays the meta but reports no change', () => {
    const thread = freshThread();
    applyAggregateToMeta(thread.meta, aggregate(1, { lastUserAction: 'U1', lastAgentAction: 'A1' }));
    expect(applyAggregateToMeta(thread.meta, aggregate(2, { lastUserAction: 'U1', lastAgentAction: 'A2' }))).toBe(false);
    expect(thread.meta.lastAgentAction).toBe('A2');
  });

  it('a last-user-action change reports a change', () => {
    const thread = freshThread();
    applyAggregateToMeta(thread.meta, aggregate(1, { lastUserAction: 'U1', lastAgentAction: 'A1' }));
    expect(applyAggregateToMeta(thread.meta, aggregate(2, { lastUserAction: 'U2', lastAgentAction: 'A1' }))).toBe(true);
    expect(thread.meta.lastUserAction).toBe('U2');
  });

  it('an aggregate that omits the agent action leaves the last one in place', () => {
    const thread = freshThread();
    applyAggregateToMeta(thread.meta, aggregate(1, { lastUserAction: 'U1', lastAgentAction: 'A1' }));
    applyAggregateToMeta(thread.meta, aggregate(2, { lastUserAction: 'U1', lastAgentAction: 'A2' }));
    applyAggregateToMeta(thread.meta, aggregate(3, { lastUserAction: 'U1' }));
    expect(thread.meta.lastAgentAction).toBe('A2');
  });
});

describe('the review tier', () => {
  it('a paused thread sorts into the call-to-action tier', () => {
    const thread = freshThread();
    expect(reviewTier(thread, 'paused')).toBe(1);
    expect(reviewTier(thread, 'failed')).toBe(1);
    expect(reviewTier(thread, 'running')).toBe(2);
    expect(reviewTier(thread, 'waiting_for_user_answer')).toBe(0);
  });
});
