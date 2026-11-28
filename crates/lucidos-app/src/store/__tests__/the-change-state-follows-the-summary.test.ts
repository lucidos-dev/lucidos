import { describe, it, expect } from 'vitest';
import {
  applyAggregateToMeta,
  changeReadyToReview,
  makeOptimisticThreadState,
  reviewTier,
  type ThreadAggregate,
  type ThreadState,
} from '../thread-events';
import { threadIsBlocked } from '../store';
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
    attentionDescendantCount: 0, archivableSubThreadCount: 0, liveEventWaitCount: 0, codingAgentChangeState: change,
    codingAgentIsExternalRepo: false, isSaved: false, hasResponse: true, lastRevivedAt: null,
    parentThreadId: null, parentThreadTitle: null, state: 'active',
  };
}

function summary(summaryVersion: number, change: CodingAgentChangeState): ThreadSummary {
  return {
    thread_id: T, title: 'Plan the fix', channel: 'claude_code', initiator: 'user',
    created_at: AT, last_activity: AT, message_count: 1, section: 'inbox',
    active_children_count: 0, total_children_count: 0, blocking_descendant_count: 0,
    attention_descendant_count: 0, archivable_sub_thread_count: 0, live_event_wait_count: 0, status: 'idle',
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

  it('reports no change for an equal state in a new object', () => {
    const thread = freshThread();
    applyAggregateToMeta(thread.meta, aggregate(1, { ...PROPOSED }));
    expect(applyAggregateToMeta(thread.meta, aggregate(2, { ...PROPOSED }))).toBe(false);
    expect(applyAggregateToMeta(thread.meta, aggregate(3, { kind: 'proposed', requires_restart: false }))).toBe(true);
  });

  it('leaves integrated hardening holds with the parent after hydration and live updates', () => {
    const hold = { kind: 'unproposed', reason: 'hardening_missing' } as const;
    const map = new Map<string, ThreadState>();
    upsertThread(map, { ...summary(1, hold), coding_agent_integrated: true }, false);
    expect(threadIsBlocked(map.get(T)!)).toBe(false);

    const live = freshThread();
    applyAggregateToMeta(live.meta, aggregate(1, hold));
    expect(threadIsBlocked(live)).toBe(true);
    applyAggregateToMeta(live.meta, { ...aggregate(2, hold), codingAgentIntegrated: true });
    expect(threadIsBlocked(live)).toBe(false);
  });

  it('follows the list read', () => {
    const map = new Map<string, ThreadState>([[T, freshThread()]]);
    upsertThread(map, summary(1, WITHHELD), false);
    expect(map.get(T)!.meta.codingAgentChangeState).toEqual(WITHHELD);

    upsertThread(map, summary(2, PROPOSED), false);
    expect(map.get(T)!.meta.codingAgentChangeState).toEqual(PROPOSED);
  });
});
