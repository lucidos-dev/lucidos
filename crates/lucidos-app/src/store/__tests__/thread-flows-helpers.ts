// Shared scaffolding for the thread-flows integration tests, split across
// thread-flows-*.test.ts. Pure builders plus the per-test seq counter,
// which each split file resets in its own beforeEach via resetSeqCounter().
import { exchangeResponseEvents, exchangeStatus, exchangeSteps, groupIntoExchanges, type Exchange, type ThreadEvent, type ThreadState } from '../thread-events';
import { handleEventWithAgg } from './aggregate-test-helper';
import { statusLabel } from '../exchange-status';

export const TS = '2026-04-17T00:00:00Z';

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

export function makeThread(id = 'thread-1', status: 'idle' | 'running' | 'waiting' = 'idle'): { map: Map<string, ThreadState>; id: string } {
  const thread: ThreadState = {
    meta: {
      id,
      title: '...',
      channel: 'chat',
      initiator: 'user',
      saved: false,
      createdAt: '',
      updatedAt: '',
      status,
      messageCount: 0,
      section: 'archived',
      activeChildrenCount: 0,
      totalChildrenCount: 0,
      blockingDescendantCount: 0, attentionDescendantCount: 0,
      codingAgentProposed: false,
      codingAgentRequiresRestart: false,
      codingAgentIsExternalRepo: false,
      codingAgentApplying: false,
      codingAgentHasDiff: false,
      lastRevivedAt: '',
      state: 'active',
      latestTodoList: null,
    },
    events: new Map(),
    streamingBuffer: '',
    eventsLoaded: true,
    eventsLoadFailed: false,
    lastDbSeq: 0,
    pendingUserMessages: [],
  };
  const map = new Map([[id, thread]]);
  return { map, id };
}

let seqCounter = 1;

/** Insert events into the thread's event map, simulating SSE arrival.
 *  Uses ascending seqs so insertion order = sort order. */
export function insertEvents(
  map: Map<string, ThreadState>,
  threadId: string,
  events: Array<ThreadEvent & { created?: string; event_id?: string }>,
): void {
  for (const event of events) {
    const created = event.created ?? TS;
    const eventId = (event as any).event_id;
    const clean = { ...event };
    delete (clean as any).created;
    delete (clean as any).event_id;
    // Use handleEventWithAgg so a synthesized ThreadAggregate (mirroring
    // backend update_thread_projection rules) is applied to thread.meta.
    handleEventWithAgg(map, threadId, seqCounter++, clean, created, eventId);
  }
}

/** Run full pipeline: events map → exchanges (oldest-first) */
export function getExchanges(map: Map<string, ThreadState>, threadId: string) {
  const thread = map.get(threadId)!;
  return groupIntoExchanges(thread.events);
}

/** Get the user-visible status label for an Exchange */
export function getLabel(ex: Exchange, streamingBuffer = '', isLast = true, hasPriorActive = false, threadIsCC = false): string {
  const status = exchangeStatus(ex, streamingBuffer, isLast, hasPriorActive, threadIsCC);
  const steps = exchangeSteps(ex);
  const events = exchangeResponseEvents(ex);
  const hasSteps = steps.length > 0 || events.some(e => e.type === 'step');
  return statusLabel(status, hasSteps).label;
}

/** Build exchanges including pending user messages (not yet in DB).
 *  @param useDisplayCreated — use `_displayCreated` instead of `created` for synthetic events
 *    (chat threads need this so sorting falls through to seq comparison). */
export function getExchangesWithPending(
  map: Map<string, ThreadState>,
  threadId: string,
  useDisplayCreated = false,
): Exchange[] {
  const thread = map.get(threadId)!;
  if (thread.pendingUserMessages.length === 0) {
    return groupIntoExchanges(thread.events);
  }
  const augmented = new Map(thread.events);
  for (let i = 0; i < thread.pendingUserMessages.length; i++) {
    const pending = thread.pendingUserMessages[i];
    const syntheticSeq = Number.MAX_SAFE_INTEGER - thread.pendingUserMessages.length + i;
    augmented.set(syntheticSeq, {
      type: 'MessageReceived' as const,
      text: pending.text,
      channel: thread.meta.channel,
      ...(useDisplayCreated
        ? { _displayCreated: pending.created }
        : { created: pending.created }),
    } as any);
  }
  return groupIntoExchanges(augmented);
}

/** Post-increment the shared seq counter (mirrors the old `seqCounter++`). */
export function nextSeq(): number {
  return seqCounter++;
}

export function resetSeqCounter(): void {
  seqCounter = 1;
}
