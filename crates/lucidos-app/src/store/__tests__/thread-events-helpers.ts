// Shared scaffolding for the thread-events tests, split across
// thread-events-*.test.ts.
import { exchangeStatus, groupIntoExchanges, handleEvent, type ThreadEvent, type ThreadMeta, type ThreadState } from '../thread-events';

export const TS = '2026-04-17T00:00:00Z';

export function makeThreadState(events: Map<number, ThreadEvent> = new Map()): ThreadState {
  const meta: ThreadMeta = {
    id: 'thread-1',
    title: 'Test Thread',
    channel: 'chat',
    initiator: 'user',
    saved: false,
    createdAt: '2026-01-01T00:00:00.000Z',
    updatedAt: '2026-01-01T00:00:00.000Z',
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
    liveEventWaitCount: 0,
    liveEventWaits: [],
  };
  return { meta, events, streamingBuffer: '', eventsLoaded: true, eventsLoadFailed: false, lastDbSeq: 0, pendingUserMessages: [] };
}

/** Helper: build a CC thread state and replay events through handleEvent,
 *  then return exchanges with their statuses. */
export function buildCCThread(events: Array<{ seq: number; event: ThreadEvent; created: string }>): {
  thread: ThreadState;
  exchanges: ReturnType<typeof groupIntoExchanges>;
  statuses: ReturnType<typeof exchangeStatus>[];
} {
  const thread = makeThreadState();
  thread.meta.channel = 'claude_code';
  const map = new Map([['t', thread]]);

  for (const { seq, event, created } of events) {
    handleEvent(map, 't', seq, event, created);
  }

  const exchanges = groupIntoExchanges(thread.events);
  const statuses = exchanges.map((exch, i) =>
    exchangeStatus(exch, '', i === exchanges.length - 1, false, true)
  );
  return { thread, exchanges, statuses };
}
