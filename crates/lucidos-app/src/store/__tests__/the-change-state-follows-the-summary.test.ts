/** ADR 0400: the live thread meta carries `codingAgentChangeState` from both
 *  summary paths, the SSE aggregate and the list read. Only a proposal reads
 *  as ready to review; withheld work does not. */
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
import type { CodingAgentChangeState, ThreadSummary } from '../../api/threads';

const T = 't-withheld';
const AT = '2026-10-02T07:04:08.000Z';
const WITHHELD: CodingAgentChangeState = { kind: 'unproposed', reason: 'turn_incomplete' };
const PROPOSED: CodingAgentChangeState = { kind: 'proposed', requires_restart: true };

function aggregate(summaryVersion: number, change: CodingAgentChangeState): ThreadAggregate {
  return {
    threadId: T, title: 'Plan the fix', channel: 'claude_code', initiator: 'user',
    createdAt: AT, lastActivity: AT, messageCount: 1, section: 'inbox', status: 'idle', summaryVersion,
    activeChildrenCount: 0, totalChildrenCount: 0, blockingDescendantCount: 0,
    attentionDescendantCount: 0, liveEventWaitCount: 0, codingAgentChangeState: change,
    codingAgentIsExternalRepo: false, isSaved: false, hasResponse: true, lastRevivedAt: null,
    parentThreadId: null, parentThreadTitle: null, state: 'active',
  };
}

function summary(summaryVersion: number, change: CodingAgentChangeState): ThreadSummary {
  return {
    thread_id: T, title: 'Plan the fix', channel: 'claude_code', initiator: 'user',
    created_at: AT, last_activity: AT, message_count: 1, section: 'inbox',
    active_children_count: 0, total_children_count: 0, blocking_descendant_count: 0,
    attention_descendant_count: 0, live_event_wait_count: 0, status: 'idle',
    summary_version: summaryVersion, coding_agent_change_state: change,
    coding_agent_is_external_repo: false,
    last_revived_at: null, state: 'active', compose_text: '', compose_images: [],
  };
}

function freshThread(): ThreadState {
  return makeOptimisticThreadState({
    id: T, title: 'Plan the fix', channel: 'claude_code', initiator: 'user',
    eventsLoaded: true, timestamp: AT, status: 'idle',
  });
}

describe('the change state on the live thread', () => {
  it('starts with no work on a new thread', () => {
    expect(freshThread().meta.codingAgentChangeState).toEqual({ kind: 'none' });
  });

  it('follows the aggregate: withheld work, then a proposal, then Apply', () => {
    const thread = freshThread();
    expect(applyAggregateToMeta(thread.meta, aggregate(1, WITHHELD))).toBe(true);
    expect(thread.meta.codingAgentChangeState).toEqual(WITHHELD);
    expect(changeReadyToReview(thread.meta)).toBe(false);
    expect(reviewTier(thread, 'idle')).toBe(2);

    applyAggregateToMeta(thread.meta, aggregate(2, PROPOSED));
    expect(changeReadyToReview(thread.meta)).toBe(true);
    expect(reviewTier(thread, 'idle')).toBe(1);

    applyAggregateToMeta(thread.meta, aggregate(3, { kind: 'none' }));
    expect(changeReadyToReview(thread.meta)).toBe(false);
  });

  // The fan-out gate in thread-sync.ts reads the changed signal, so a state
  // that says the same thing must not report a change.
  it('reports no change for an equal state in a new object', () => {
    const thread = freshThread();
    applyAggregateToMeta(thread.meta, aggregate(1, { ...PROPOSED }));
    expect(applyAggregateToMeta(thread.meta, aggregate(2, { ...PROPOSED }))).toBe(false);
    expect(applyAggregateToMeta(thread.meta, aggregate(3, { kind: 'proposed', requires_restart: false }))).toBe(true);
  });

  it('follows the list read', () => {
    const map = new Map<string, ThreadState>([[T, freshThread()]]);
    upsertThread(map, summary(1, WITHHELD), false);
    expect(map.get(T)!.meta.codingAgentChangeState).toEqual(WITHHELD);

    upsertThread(map, summary(2, PROPOSED), false);
    expect(map.get(T)!.meta.codingAgentChangeState).toEqual(PROPOSED);
  });
});
