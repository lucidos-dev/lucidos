/**
 * Stop-clears-queue: pressing Stop on a chat thread with queued follow-ups must
 * (1) retract each un-injected queued message via `/chat/queued-message/remove`
 * BEFORE cancelling (so the backend `filter_removed_queued_prompts` drops it at
 * loop finalize instead of re-running it above "Response canceled"), (2) return
 * the retracted texts to the compose box in FIFO order, and (3) then cancel.
 * Already-injected messages (409) stay under the cancelled exchange — not moved
 * to compose. See docs/plans/2026-07-19-stop-clears-queued-messages.md.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';

// Spy on the compose write so we can assert the appended text without the 250ms
// debounced PUT firing a stray fetch. (vi.hoisted: the factory is hoisted above imports.)
const { updateComposeSpy } = vi.hoisted(() => ({ updateComposeSpy: vi.fn() }));
vi.mock('./compose', async (importOriginal) => ({
  ...(await importOriginal<typeof import('./compose')>()),
  updateCompose: updateComposeSpy,
}));
// The trash-button wrapper re-syncs on failure; the orchestration core doesn't,
// but mock it so no path hits a real fetch.
const { refreshThreadEvents } = vi.hoisted(() => ({ refreshThreadEvents: vi.fn(async (_id: string) => {}) }));
vi.mock('./thread-loading', async (importOriginal) => ({
  ...(await importOriginal<typeof import('./thread-loading')>()),
  refreshThreadEvents,
}));

import { cancelCurrentExchange, removeQueuedMessage } from './chat';
import { focusedThreadId, threadMap, cancelingThreadIds, removingQueuedMessageIds, queuedMessageRemovalKey } from '../store';
import { setDraft, _resetComposeDraftsForTesting } from '../composeDrafts';
import type { StoredEvent, ThreadState } from '../thread-events';

const TS = '2026-07-19T00:00:00Z';
const originalFetch = globalThis.fetch;

/** A running chat thread with one active streaming turn + N queued follow-ups
 *  (as optimistic pending messages, which computeExchanges folds into stepless
 *  queued exchanges — the same shape a persisted queued MessageReceived takes). */
function makeQueuedThread(pending: Array<{ text: string; eventId: string }>): ThreadState {
  return {
    meta: {
      id: 't-1', title: '', channel: 'chat', initiator: 'user', saved: false,
      createdAt: '', updatedAt: '', status: 'running',
      codingAgentProposed: false, codingAgentRequiresRestart: false,
      codingAgentIsExternalRepo: false, codingAgentApplying: false, codingAgentHasDiff: false,
      lastRevivedAt: '', messageCount: 0, section: 'archived',
      activeChildrenCount: 0, totalChildrenCount: 0,
      blockingDescendantCount: 0, attentionDescendantCount: 0,
      state: 'active', latestTodoList: null,
    },
    events: new Map<number, StoredEvent>([
      [1, { type: 'MessageReceived', text: 'active', created: TS, _eventId: 'mr-active', channel: 'chat' } as StoredEvent],
      [2, { type: 'TextStreamed', text: 'working', created: TS } as StoredEvent],
    ]),
    streamingBuffer: '',
    eventsLoaded: true,
    eventsLoadFailed: false,
    lastDbSeq: 2,
    pendingUserMessages: pending.map((p, i) => ({
      text: p.text, eventId: p.eventId, created: `2026-07-19T00:00:0${i + 2}Z`,
    })),
  };
}

function setThread(pending: Array<{ text: string; eventId: string }>) {
  const map = new Map<string, ThreadState>();
  map.set('t-1', makeQueuedThread(pending));
  threadMap.value = map;
}

interface Call { url: string; body: unknown }

/** Route fetch by URL. `removed409` message_ids return 409 (already injected);
 *  `removeThrows` message_ids reject (transport). Records call order. */
function installFetch(calls: Call[], opts: { removed409?: Set<string>; removeThrows?: Set<string> } = {}) {
  const mock = vi.fn(async (url: string, init?: { body?: string }) => {
    const body = init?.body ? JSON.parse(init.body) : undefined;
    calls.push({ url, body });
    if (url.includes('/chat/queued-message/remove')) {
      const id = body?.message_id as string;
      if (opts.removeThrows?.has(id)) throw new TypeError('Load failed');
      if (opts.removed409?.has(id)) return new Response(null, { status: 409 });
      return new Response(null, { status: 200 });
    }
    if (url.includes('/chat/cancel')) {
      return new Response(JSON.stringify({ canceled: true }), { status: 200 });
    }
    return new Response(null, { status: 200 });
  });
  globalThis.fetch = mock as unknown as typeof fetch;
  return mock;
}

function removeCalls(calls: Call[]): string[] {
  return calls.filter(c => c.url.includes('/chat/queued-message/remove')).map(c => (c.body as { message_id: string }).message_id);
}
function cancelIndex(calls: Call[]): number {
  return calls.findIndex(c => c.url.includes('/chat/cancel'));
}

describe('cancelCurrentExchange — stop clears queued messages to compose', () => {
  beforeEach(() => {
    focusedThreadId.value = 't-1';
    cancelingThreadIds.value = new Set();
    removingQueuedMessageIds.value = new Set();
    _resetComposeDraftsForTesting();
    updateComposeSpy.mockClear();
    refreshThreadEvents.mockClear();
  });

  afterEach(() => {
    globalThis.fetch = originalFetch;
    focusedThreadId.value = null;
    threadMap.value = new Map();
    cancelingThreadIds.value = new Set();
    removingQueuedMessageIds.value = new Set();
    _resetComposeDraftsForTesting();
    vi.restoreAllMocks();
  });

  it('retracts every queued message BEFORE cancelling and appends their text FIFO', async () => {
    setThread([{ text: 'follow 1', eventId: 'q1' }, { text: 'follow 2', eventId: 'q2' }]);
    const calls: Call[] = [];
    installFetch(calls);

    const outcome = await cancelCurrentExchange('t-1');

    expect(outcome).toBe('canceled');
    // Both retracted, in FIFO order, and all before the cancel.
    expect(removeCalls(calls)).toEqual(['q1', 'q2']);
    const cancelAt = cancelIndex(calls);
    expect(cancelAt).toBe(2); // after both removes
    // Their text returned to compose, FIFO, blank-line separated.
    expect(updateComposeSpy).toHaveBeenCalledWith('t-1', { text: 'follow 1\n\nfollow 2' });
  });

  it('appends after an existing compose draft', async () => {
    setThread([{ text: 'follow 1', eventId: 'q1' }]);
    setDraft('t-1', { text: 'my draft', image_hashes: [], mode: null });
    installFetch([]);

    await cancelCurrentExchange('t-1');

    expect(updateComposeSpy).toHaveBeenCalledWith('t-1', { text: 'my draft\n\nfollow 1' });
  });

  it('excludes an already-injected (409) message from compose but still cancels', async () => {
    setThread([{ text: 'follow 1', eventId: 'q1' }, { text: 'follow 2', eventId: 'q2' }]);
    const calls: Call[] = [];
    installFetch(calls, { removed409: new Set(['q2']) });

    const outcome = await cancelCurrentExchange('t-1');

    expect(outcome).toBe('canceled');
    // q2 was already injected → part of the cancelled response, not moved to compose.
    expect(updateComposeSpy).toHaveBeenCalledWith('t-1', { text: 'follow 1' });
    expect(cancelIndex(calls)).toBeGreaterThanOrEqual(0);
  });

  it('a failed retract is excluded from compose; the cancel still fires', async () => {
    setThread([{ text: 'follow 1', eventId: 'q1' }, { text: 'follow 2', eventId: 'q2' }]);
    const calls: Call[] = [];
    installFetch(calls, { removeThrows: new Set(['q1']) });

    const outcome = await cancelCurrentExchange('t-1');

    expect(outcome).toBe('canceled');
    expect(updateComposeSpy).toHaveBeenCalledWith('t-1', { text: 'follow 2' });
    expect(cancelIndex(calls)).toBeGreaterThanOrEqual(0);
  });

  it('when every retract fails, nothing is appended to compose', async () => {
    setThread([{ text: 'follow 1', eventId: 'q1' }]);
    const calls: Call[] = [];
    installFetch(calls, { removeThrows: new Set(['q1']) });

    await cancelCurrentExchange('t-1');

    expect(updateComposeSpy).not.toHaveBeenCalled();
    expect(cancelIndex(calls)).toBeGreaterThanOrEqual(0);
  });

  it('trash-then-Stop: Stop awaits the in-flight removal and never appends a failed one', async () => {
    // The user clicks a queue item's trash, then immediately presses Stop while
    // that removal is still in flight. Stop must AWAIT the shared removal's real
    // outcome — not assume success — so a removal that then fails is not appended
    // to compose (which would duplicate it, and it would also re-run on cancel).
    setThread([{ text: 'follow 1', eventId: 'q1' }]);
    let rejectRemove!: (e: unknown) => void;
    const removePending = new Promise<never>((_res, rej) => { rejectRemove = rej; });
    globalThis.fetch = vi.fn(async (url: string) => {
      if (url.includes('/chat/queued-message/remove')) { await removePending; return new Response(null, { status: 200 }); }
      if (url.includes('/chat/cancel')) return new Response(JSON.stringify({ canceled: true }), { status: 200 });
      return new Response(null, { status: 200 });
    }) as unknown as typeof fetch;

    const trash = removeQueuedMessage('t-1', 'q1'); // in-flight (deferred fetch)
    const stop = cancelCurrentExchange('t-1');       // shares the same in-flight removal
    rejectRemove(new TypeError('Load failed'));       // the removal fails
    await Promise.allSettled([trash, stop]);

    // Failed removal → not moved to compose (no duplication).
    expect(updateComposeSpy).not.toHaveBeenCalled();
    // Only ONE remove request went out (the two callers deduped onto it).
    const removeCallCount = (globalThis.fetch as unknown as ReturnType<typeof vi.fn>).mock.calls
      .filter((c: unknown[]) => String(c[0]).includes('/chat/queued-message/remove')).length;
    expect(removeCallCount).toBe(1);
  });

  it('excludes a message already being trashed (mid-removal) from the Stop clear', async () => {
    // A message the user already clicked trash on is hidden from the queued group
    // by the UI; Stop must mirror that and NOT resurface it into compose (the
    // trash-then-Stop SUCCESS branch — its own removal owns it).
    setThread([{ text: 'follow 1', eventId: 'q1' }, { text: 'follow 2', eventId: 'q2' }]);
    removingQueuedMessageIds.value = new Set([queuedMessageRemovalKey('t-1', 'q1')]);
    const calls: Call[] = [];
    installFetch(calls);

    await cancelCurrentExchange('t-1');

    // q1 is mid-trash → excluded; only q2 is retracted and returned to compose.
    expect(removeCalls(calls)).toEqual(['q2']);
    expect(updateComposeSpy).toHaveBeenCalledWith('t-1', { text: 'follow 2' });
  });

  it('no queued messages → cancels directly, no retract, no compose write', async () => {
    setThread([]); // active streaming turn only, nothing queued
    const calls: Call[] = [];
    installFetch(calls);

    const outcome = await cancelCurrentExchange('t-1');

    expect(outcome).toBe('canceled');
    expect(removeCalls(calls)).toEqual([]);
    expect(updateComposeSpy).not.toHaveBeenCalled();
    expect(cancelIndex(calls)).toBe(0);
  });
});
