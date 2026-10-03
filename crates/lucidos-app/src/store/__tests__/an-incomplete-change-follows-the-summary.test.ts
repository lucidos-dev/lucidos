/** I4 of ADR 0346: the live thread meta carries `codingAgentIncomplete` from
 *  both summary paths, the SSE aggregate and the list read. Every "ready to
 *  review" reader then passes the change over while it still blocks Archive. */
import { describe, it, expect } from 'vitest';
import {
  applyAggregateToMeta,
  changeReadyToReview,
  makeOptimisticThreadState,
  reviewTier,
  type ThreadAggregate,
  type ThreadState,
} from '../thread-events';
import { upsertThread } from '../actions/thread-loading';
import type { ThreadSummary } from '../../api/threads';

const T = 't-stopped';
const AT = '2026-10-02T07:04:08.000Z';

function aggregate(summaryVersion: number, incomplete: boolean, proposed = true): ThreadAggregate {
  return {
    threadId: T, title: 'Plan the fix', channel: 'claude_code', initiator: 'user',
    createdAt: AT, lastActivity: AT, messageCount: 1, section: 'inbox', status: 'idle', summaryVersion,
    activeChildrenCount: 0, totalChildrenCount: 0, blockingDescendantCount: 0,
    attentionDescendantCount: 0, liveEventWaitCount: 0, codingAgentHasDiff: true,
    codingAgentProposed: proposed, codingAgentRequiresRestart: false, codingAgentIncomplete: incomplete,
    codingAgentIsExternalRepo: false, isSaved: false, hasResponse: true, lastRevivedAt: null,
    parentThreadId: null, parentThreadTitle: null, state: 'active',
  };
}

function summary(summaryVersion: number, incomplete: boolean): ThreadSummary {
  return {
    thread_id: T, title: 'Plan the fix', channel: 'claude_code', initiator: 'user',
    created_at: AT, last_activity: AT, message_count: 1, section: 'inbox',
    active_children_count: 0, total_children_count: 0, blocking_descendant_count: 0,
    attention_descendant_count: 0, live_event_wait_count: 0, status: 'idle',
    summary_version: summaryVersion, coding_agent_has_diff: true, coding_agent_proposed: true,
    coding_agent_requires_restart: false, coding_agent_is_external_repo: false,
    coding_agent_incomplete: incomplete,
    last_revived_at: null, state: 'active', compose_text: '', compose_images: [],
  };
}

function freshThread(): ThreadState {
  return makeOptimisticThreadState({
    id: T, title: 'Plan the fix', channel: 'claude_code', initiator: 'user',
    eventsLoaded: true, timestamp: AT, status: 'idle',
  });
}

describe('an incomplete change on the live thread', () => {
  it('follows the aggregate: a Stop marks it, a clean finish clears it, Apply drops it', () => {
    const thread = freshThread();
    applyAggregateToMeta(thread.meta, aggregate(1, true));
    expect(thread.meta.codingAgentIncomplete).toBe(true);
    expect(changeReadyToReview(thread.meta)).toBe(false);
    expect(reviewTier(thread, 'idle')).toBe(2);

    applyAggregateToMeta(thread.meta, aggregate(2, false));
    expect(changeReadyToReview(thread.meta)).toBe(true);
    expect(reviewTier(thread, 'idle')).toBe(1);

    applyAggregateToMeta(thread.meta, aggregate(3, false, false));
    expect(changeReadyToReview(thread.meta)).toBe(false);
  });

  it('follows the list read', () => {
    const map = new Map<string, ThreadState>([[T, freshThread()]]);
    upsertThread(map, summary(1, true), false);
    expect(map.get(T)!.meta.codingAgentIncomplete).toBe(true);
    expect(map.get(T)!.meta.codingAgentProposed).toBe(true);

    upsertThread(map, summary(2, false), false);
    expect(map.get(T)!.meta.codingAgentIncomplete).toBe(false);
  });
});
