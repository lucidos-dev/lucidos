/** The client never replaces its thread status with an older summary.
 *
 *  Every summary carries `summaryVersion`, which the engine bumps on each
 *  change to the row. So arrival order stops mattering: a reordered broadcast,
 *  a duplicate delivery and a slow resync read all lose to what the client
 *  already holds.
 *
 *  The last block replays the reported race. A coding-agent follow-up lands
 *  its first steps, then a resync read taken before them arrives saying idle.
 *  The turn must keep reading "Working", never flash "Done".
 */
import { describe, it, expect } from 'vitest';
import {
  UNVERSIONED,
  applyAggregateToMeta,
  exchangeStatus,
  groupIntoExchanges,
  handleEvent,
  makeOptimisticThreadState,
  type ThreadAggregate,
  type ThreadEvent,
  type ThreadState,
} from '../thread-events';
import { isRenderedThreadIdle } from '../store';
import { upsertThread } from '../actions/thread-loading';
import type { ThreadSummary } from '../../api/threads';

const T = 't1';
const AT = '2026-09-29T12:00:00.000Z';

function aggregate(summaryVersion: number, status: ThreadAggregate['status']): ThreadAggregate {
  return {
    threadId: T, title: 'Fix the bug', channel: 'claude_code', initiator: 'user',
    createdAt: AT, lastActivity: AT, messageCount: 1, section: 'inbox', status, summaryVersion,
    activeChildrenCount: 0, totalChildrenCount: 0, blockingDescendantCount: 0,
    attentionDescendantCount: 0, liveEventWaitCount: 0, codingAgentHasDiff: false,
    codingAgentProposed: false, codingAgentRequiresRestart: false, codingAgentIncomplete: false,
    codingAgentIsExternalRepo: false, isSaved: false, hasResponse: true, lastRevivedAt: null,
    parentThreadId: null, parentThreadTitle: null, state: 'active',
  };
}

function summary(summaryVersion: number, status: ThreadSummary['status']): ThreadSummary {
  return {
    thread_id: T, title: 'Fix the bug', channel: 'claude_code', initiator: 'user',
    created_at: AT, last_activity: AT, message_count: 1, section: 'inbox',
    active_children_count: 0, total_children_count: 0, blocking_descendant_count: 0,
    attention_descendant_count: 0, live_event_wait_count: 0, status, summary_version: summaryVersion,
    coding_agent_has_diff: false, coding_agent_proposed: false,
    coding_agent_requires_restart: false, coding_agent_is_external_repo: false,
    coding_agent_incomplete: false,
    last_revived_at: null, state: 'active', compose_text: '', compose_images: [],
  };
}

function freshThread(): ThreadState {
  return makeOptimisticThreadState({
    id: T, title: 'Fix the bug', channel: 'claude_code', initiator: 'user',
    eventsLoaded: true, timestamp: AT, status: 'idle',
  });
}

describe('the versioned apply', () => {
  it('lets the first server summary replace a row the client drew itself', () => {
    const thread = freshThread();
    expect(thread.meta.summaryVersion).toBe(UNVERSIONED);
    applyAggregateToMeta(thread.meta, aggregate(0, 'running'));
    expect(thread.meta.status).toBe('running');
  });

  it('refuses an aggregate broadcast out of order', () => {
    const thread = freshThread();
    applyAggregateToMeta(thread.meta, aggregate(7, 'idle'));
    const changed = applyAggregateToMeta(thread.meta, aggregate(6, 'running'));
    expect(changed).toBe(false);
    expect(thread.meta.status).toBe('idle');
    expect(thread.meta.summaryVersion).toBe(7);
  });

  it('treats a duplicate delivery as the same state', () => {
    const thread = freshThread();
    applyAggregateToMeta(thread.meta, aggregate(7, 'running'));
    expect(applyAggregateToMeta(thread.meta, aggregate(7, 'running'))).toBe(false);
    expect(thread.meta.status).toBe('running');
  });

  it('takes a summary that carries no version rather than refusing it forever', () => {
    const map = new Map([[T, freshThread()]]);
    applyAggregateToMeta(map.get(T)!.meta, aggregate(7, 'running'));
    const { summary_version: _dropped, ...versionless } = summary(0, 'idle');
    upsertThread(map, versionless as ThreadSummary, false);
    expect(map.get(T)!.meta.status).toBe('idle');
  });

  it('refuses a list read older than the live state, and takes a newer one', () => {
    const map = new Map([[T, freshThread()]]);
    applyAggregateToMeta(map.get(T)!.meta, aggregate(7, 'running'));

    upsertThread(map, summary(6, 'idle'), false);
    expect(map.get(T)!.meta.status).toBe('running');

    upsertThread(map, summary(8, 'idle'), false);
    expect(map.get(T)!.meta.status).toBe('idle');
  });
});

/** The reported race, event by event, with the aggregates the engine sends. */
describe('a follow-up never flashes Done over a stale idle read', () => {
  const FOLLOW_UP: Array<[number, ThreadEvent, number]> = [
    [1, { type: 'MessageReceived', text: 'fix bug', channel: 'claude_code' } as ThreadEvent, 1],
    [2, { type: 'SessionStarted', session_id: 's1' } as ThreadEvent, 2],
    [3, { type: 'CodingAgentIdled', has_changes: false } as ThreadEvent, 3],
    [4, { type: 'MessageReceived', text: 'also the tests', channel: 'claude_code' } as ThreadEvent, 4],
    [5, { type: 'CodingAgentPromptSent', text: 'also the tests' } as ThreadEvent, 5],
    [6, { type: 'CodingAgentToolCalled', name: 'Edit', args: {} } as ThreadEvent, 6],
  ];
  const STATUS_AFTER: Record<number, ThreadAggregate['status']> = { 1: 'running', 2: 'running', 3: 'idle', 4: 'running', 5: 'running', 6: 'running' };

  it('keeps the follow-up working when a read taken at the idle arrives late', () => {
    const map = new Map([[T, freshThread()]]);
    for (const [seq, event, version] of FOLLOW_UP) {
      handleEvent(map, T, seq, event, AT, `e${seq}`, aggregate(version, STATUS_AFTER[seq]));
    }
    // A resync read taken while the thread sat idle after the first turn.
    upsertThread(map, summary(3, 'idle'), false);

    const thread = map.get(T)!;
    expect(thread.meta.status).toBe('running');
    const exchanges = groupIntoExchanges(thread.events);
    const last = exchanges[exchanges.length - 1];
    const status = exchangeStatus(last, '', true, false, true, isRenderedThreadIdle(thread), false);
    expect(status).toBe('coding-agent-working');
  });
});
