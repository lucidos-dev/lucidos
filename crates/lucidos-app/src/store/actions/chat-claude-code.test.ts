/**
 * Tests that change state transitions (applied, discarded, reverted) are stored
 * correctly as thread events. Toast notifications for these transitions are
 * handled in handleThreadEvent (thread-sync.ts) by reacting to ChangeApplied,
 * ChangeDiscarded, and ChangeReverted SSE events — regardless of source
 * (Changes panel, CC idle banner, auto-apply, etc.).
 */
import { describe, it, expect } from 'vitest';

import { handleEvent, type ThreadState } from '../thread-events';

function makeThread(): { map: Map<string, ThreadState>; id: string } {
  const id = 'thread-1';
  const map = new Map<string, ThreadState>();
  map.set(id, {
    meta: { id, title: 'Test', channel: 'claude_code', initiator: 'user', saved: false, createdAt: '', updatedAt: '', status: 'idle', codingAgentProposed: false, codingAgentRequiresRestart: false, codingAgentIsExternalRepo: false, codingAgentApplying: false, codingAgentHasDiff: false, lastRevivedAt: '', messageCount: 0, section: 'archived', activeChildrenCount: 0, totalChildrenCount: 0, blockingDescendantCount: 0, attentionDescendantCount: 0, state: 'active', latestTodoList: null, liveEventWaits: [] },
    events: new Map(),
    streamingBuffer: '',
    eventsLoaded: false,
    eventsLoadFailed: false,
    lastDbSeq: 0,
    pendingUserMessages: [],
  });
  return { map, id };
}

const TS = '2026-04-17T00:00:00Z';

describe('Change events stored in thread', () => {
  it('ChangeApplied is stored as a thread event', () => {
    const { map, id } = makeThread();
    handleEvent(map, id, 1, { type: 'MessageReceived', text: 'fix it' }, TS);
    handleEvent(map, id, 2, { type: 'SessionStarted', session_id: 's1' }, TS);
    handleEvent(map, id, 3, { type: 'ChangeProposed', change_id: 'c-1', description: 'Fix' }, TS);
    handleEvent(map, id, 4, { type: 'ChangeApplied', change_id: 'c-1' }, TS);

    const events = [...map.get(id)!.events.values()];
    expect(events.some(e => e.type === 'ChangeApplied')).toBe(true);
  });

  it('ChangeDiscarded is stored as a thread event', () => {
    const { map, id } = makeThread();
    handleEvent(map, id, 1, { type: 'MessageReceived', text: 'try it' }, TS);
    handleEvent(map, id, 2, { type: 'SessionStarted', session_id: 's1' }, TS);
    handleEvent(map, id, 3, { type: 'ChangeProposed', change_id: 'c-1', description: 'Experiment' }, TS);
    handleEvent(map, id, 4, { type: 'ChangeDiscarded', change_id: 'c-1' }, TS);

    const events = [...map.get(id)!.events.values()];
    expect(events.some(e => e.type === 'ChangeDiscarded')).toBe(true);
  });

  it('ChangeReverted is stored as a thread event', () => {
    const { map, id } = makeThread();
    handleEvent(map, id, 1, { type: 'MessageReceived', text: 'fix it' }, TS);
    handleEvent(map, id, 2, { type: 'SessionStarted', session_id: 's1' }, TS);
    handleEvent(map, id, 3, { type: 'ChangeProposed', change_id: 'c-1', description: 'Fix' }, TS);
    handleEvent(map, id, 4, { type: 'ChangeApplied', change_id: 'c-1' }, TS);
    handleEvent(map, id, 5, { type: 'ChangeReverted', change_id: 'c-1' }, TS);

    const events = [...map.get(id)!.events.values()];
    expect(events.some(e => e.type === 'ChangeReverted')).toBe(true);
  });
});
