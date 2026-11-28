import { describe, it, expect } from 'vitest';
import {
  applyAggregateToMeta,
  makeOptimisticThreadState,
  NO_CHANGE,
  type ThreadAggregate,
  type ThreadState,
} from '../thread-events';
import { upsertThread } from '../actions/thread-loading';
import type { ThreadSummary } from '../../api/threads';

const T = 't-family';
const AT = '2026-11-24T09:00:00.000Z';

function aggregate(summaryVersion: number, archivableSubThreadCount: number): ThreadAggregate {
  return {
    threadId: T, title: 'Family', channel: 'chat', initiator: 'user',
    createdAt: AT, lastActivity: AT, messageCount: 1, section: 'inbox', status: 'idle', summaryVersion,
    activeChildrenCount: 0, totalChildrenCount: 3, blockingDescendantCount: 0,
    attentionDescendantCount: 0, archivableSubThreadCount, liveEventWaitCount: 0,
    codingAgentChangeState: NO_CHANGE, codingAgentIsExternalRepo: false, isSaved: false,
    hasResponse: true, lastRevivedAt: null, parentThreadId: null, parentThreadTitle: null,
    state: 'active',
  };
}

function summary(summaryVersion: number, archivableSubThreadCount: number): ThreadSummary {
  return {
    thread_id: T, title: 'Family', channel: 'chat', initiator: 'user',
    created_at: AT, last_activity: AT, message_count: 1, section: 'inbox',
    active_children_count: 0, total_children_count: 3, blocking_descendant_count: 0,
    attention_descendant_count: 0, archivable_sub_thread_count: archivableSubThreadCount,
    live_event_wait_count: 0, status: 'idle', summary_version: summaryVersion,
    coding_agent_change_state: NO_CHANGE, coding_agent_is_external_repo: false,
    last_revived_at: null, state: 'active', compose_text: '', compose_images: [],
  };
}

function freshThread(): ThreadState {
  return makeOptimisticThreadState({
    id: T, title: 'Family', channel: 'chat', initiator: 'user',
    eventsLoaded: true, timestamp: AT, status: 'idle',
  });
}

describe('the archivable sub-thread count on the live thread', () => {
  it('starts at zero on a new thread', () => {
    expect(freshThread().meta.archivableSubThreadCount).toBe(0);
  });

  it('follows the aggregate frame and reports the change', () => {
    const thread = freshThread();
    expect(applyAggregateToMeta(thread.meta, aggregate(1, 2))).toBe(true);
    expect(thread.meta.archivableSubThreadCount).toBe(2);
    expect(applyAggregateToMeta(thread.meta, aggregate(2, 0))).toBe(true);
    expect(thread.meta.archivableSubThreadCount).toBe(0);
  });

  it('ignores a stale aggregate', () => {
    const thread = freshThread();
    applyAggregateToMeta(thread.meta, aggregate(5, 2));
    applyAggregateToMeta(thread.meta, aggregate(4, 7));
    expect(thread.meta.archivableSubThreadCount).toBe(2);
  });

  it('arrives with the summary on a new row and on a refreshed one', () => {
    const map = new Map<string, ThreadState>();
    upsertThread(map, summary(1, 3), false);
    expect(map.get(T)!.meta.archivableSubThreadCount).toBe(3);
    upsertThread(map, summary(2, 1), false);
    expect(map.get(T)!.meta.archivableSubThreadCount).toBe(1);
  });
});
