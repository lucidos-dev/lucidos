/** Stop restores only inputs the engine took back, without client retract calls. */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';

// Spy on compose writes to verify report reconciliation never appends locally
// or schedules a second PUT. (vi.hoisted: the factory is hoisted above imports.)
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

import { cancelCurrentExchange, handleCancelExchange, editQueuedMessage } from './chat';
import { focusedThreadId, threadMap, cancelingThreadIds, removingQueuedMessageIds, queuedMessageRemovalKey, toasts } from '../store';
import { getDraft, setDraft, _resetComposeDraftsForTesting } from '../composeDrafts';
import { acceptComposeReport, parseComposeReport, _resetComposeSyncForTesting } from '../composeSync';
import type { StoredEvent, ThreadState } from '../thread-events';

const TS = '2026-07-19T00:00:00Z';
const originalFetch = globalThis.fetch;

/** A running chat thread with one active streaming turn + N queued follow-ups
 *  (as optimistic pending messages, which computeExchanges folds into stepless
 *  queued exchanges — the same shape a persisted queued MessageReceived takes). */
type QueuedFixture = { text: string; eventId: string; image_hashes?: string[] };

function makeQueuedThread(pending: QueuedFixture[]): ThreadState {
  return {
    meta: {
      id: 't-1', title: '', channel: 'chat', initiator: 'user', saved: false,
      createdAt: '', updatedAt: '', summaryVersion: 0, status: 'running',
      codingAgentChangeState: { kind: 'none' },
      codingAgentIsExternalRepo: false,
      lastRevivedAt: '', messageCount: 0, section: 'archived',
      activeChildrenCount: 0, totalChildrenCount: 0,
      blockingDescendantCount: 0, attentionDescendantCount: 0,
      state: 'active', latestTodoList: null,
      liveEventWaitCount: 0,
      liveEventWaits: [],
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
      text: p.text, eventId: p.eventId, created: `2026-07-19T00:00:0${i + 2}Z`, image_hashes: p.image_hashes,
    })),
  };
}

function setThread(pending: QueuedFixture[]) {
  const map = new Map<string, ThreadState>();
  map.set('t-1', makeQueuedThread(pending));
  threadMap.value = map;
}


interface TakenBack { thread_id?: string; id: string; text: string; image_hashes: string[] }

function installCancel(takenBack: TakenBack[], failed: string[] = [], canceled = true) {
  const drafts = new Map<string, { text: string; images: string[]; revision: number }>();
  const messages = takenBack.map(m => {
    const threadId = m.thread_id ?? 't-1';
    const prior = drafts.get(threadId) ?? { text: getDraft(threadId).text, images: getDraft(threadId).image_hashes, revision: 0 };
    const text = m.text ? [prior.text, m.text].filter(Boolean).join('\n\n') : prior.text;
    const images = [...new Set([...prior.images, ...m.image_hashes])];
    const revision = prior.revision + 1;
    drafts.set(threadId, { text, images, revision });
    return { thread_id: threadId, ...m, compose: { summary_version: revision, compose_epoch: revision, text, image_hashes: images } };
  });
  const fetch = vi.fn<(url: string, init?: RequestInit) => Promise<Response>>().mockImplementation(async () => new Response(JSON.stringify({
    canceled, taken_back_messages: messages, failed_take_back_ids: failed,
  }), { status: 200 }));
  globalThis.fetch = fetch as unknown as typeof globalThis.fetch;
  return fetch;
}

describe('Stop returns engine-owned take-backs to compose', () => {
  beforeEach(() => {
    focusedThreadId.value = 't-1';
    cancelingThreadIds.value = new Set();
    removingQueuedMessageIds.value = new Set();
    toasts.value = [];
    _resetComposeDraftsForTesting();
    _resetComposeSyncForTesting();
    updateComposeSpy.mockClear();
    refreshThreadEvents.mockClear();
    setThread([]);
  });

  afterEach(() => {
    globalThis.fetch = originalFetch;
    focusedThreadId.value = null;
    threadMap.value = new Map();
    cancelingThreadIds.value = new Set();
    removingQueuedMessageIds.value = new Set();
    _resetComposeDraftsForTesting();
    _resetComposeSyncForTesting();
    vi.restoreAllMocks();
  });

  it('restores server-confirmed text in FIFO order with one cancel request', async () => {
    const fetch = installCancel([
      { id: 'q1', text: 'follow 1', image_hashes: [] },
      { id: 'q2', text: 'follow 2', image_hashes: [] },
    ]);
    expect(await cancelCurrentExchange('t-1')).toBe('canceled');
    expect(fetch).toHaveBeenCalledTimes(1);
    expect(String(fetch.mock.calls[0][0])).toContain('/chat/cancel');
    expect(getDraft('t-1').text).toBe('follow 1\n\nfollow 2');
    expect(updateComposeSpy).not.toHaveBeenCalled();
  });

  it('appends text and deduplicated images after the existing draft', async () => {
    setDraft('t-1', { text: 'draft', image_hashes: ['img-a'], mode: null });
    installCancel([
      { id: 'q1', text: 'follow', image_hashes: ['img-a', 'img-b'] },
      { id: 'q2', text: '', image_hashes: ['img-b', 'img-c'] },
    ]);
    await cancelCurrentExchange('t-1');
    expect(getDraft('t-1')).toMatchObject({ text: 'draft\n\nfollow', image_hashes: ['img-a', 'img-b', 'img-c'] });
    expect(updateComposeSpy).not.toHaveBeenCalled();
  });

  it('reconciles duplicate and reversed SSE/HTTP reports without appending', async () => {
    acceptComposeReport('t-1', parseComposeReport({ summary_version: 3, compose_epoch: 2, text: 'newer draft' }));
    installCancel([{ id: 'q1', text: 'older restoration', image_hashes: [] }]);
    await cancelCurrentExchange('t-1');
    await cancelCurrentExchange('t-1');
    expect(getDraft('t-1').text).toBe('newer draft');
    expect(updateComposeSpy).not.toHaveBeenCalled();
  });

  it('recovers a lost cancel response from a subsequent compose report', async () => {
    globalThis.fetch = vi.fn(async () => { throw new TypeError('Load failed'); }) as unknown as typeof fetch;
    expect(await cancelCurrentExchange('t-1')).toBe('failed');
    acceptComposeReport('t-1', parseComposeReport({ summary_version: 2, compose_epoch: 1, text: 'durable restored input', image_hashes: ['image'] }));
    expect(getDraft('t-1')).toMatchObject({ text: 'durable restored input', image_hashes: ['image'] });
    expect(updateComposeSpy).not.toHaveBeenCalled();
  });

  it('does not append an Edit after Stop won its removal race', async () => {
    globalThis.fetch = vi.fn(async () => new Response(JSON.stringify({ compose: {
      summary_version: 2, compose_epoch: 1, text: 'restored once', image_hashes: ['image'],
    } }))) as unknown as typeof fetch;
    await editQueuedMessage('t-1', { id: 'q1', text: 'restored once', imageHashes: ['image'] });
    expect(getDraft('t-1')).toMatchObject({ text: 'restored once', image_hashes: ['image'] });
    expect(updateComposeSpy).not.toHaveBeenCalled();
  });

  it('does not restore a consumed input omitted by the engine', async () => {
    setThread([{ eventId: 'consumed', text: 'already read' }]);
    installCancel([]);
    await cancelCurrentExchange('t-1');
    expect(updateComposeSpy).not.toHaveBeenCalled();
  });

  it('reports failed take-backs and restores only successes', async () => {
    installCancel([{ id: 'q2', text: 'success', image_hashes: [] }], ['q1']);
    await cancelCurrentExchange('t-1');
    expect(getDraft('t-1').text).toBe('success');
    expect(toasts.value.some(t => t.type === 'error' && t.message.includes('may still run'))).toBe(true);
  });

  it("reconciles durable Stop restoration even with an in-flight trash request", async () => {
    removingQueuedMessageIds.value = new Set([queuedMessageRemovalKey('t-1', 'q1')]);
    installCancel([{ id: 'q1', text: 'trashed', image_hashes: [] }]);
    await cancelCurrentExchange('t-1');
    expect(updateComposeSpy).not.toHaveBeenCalled();
  });

  it('clears optimistic pending rows for confirmed take-backs', async () => {
    setThread([{ eventId: 'q1', text: 'still requesting' }]);
    installCancel([{ id: 'q1', text: 'still requesting', image_hashes: [] }]);
    await cancelCurrentExchange('t-1');
    expect(threadMap.value.get('t-1')!.pendingUserMessages).toEqual([]);
  });

  it('restores each thread separately for an unscoped workspace Stop', async () => {
    focusedThreadId.value = null;
    installCancel([
      { id: 'q1', thread_id: 't-1', text: 'first thread', image_hashes: [] },
      { id: 'q2', thread_id: 't-2', text: 'second thread', image_hashes: [] },
    ]);
    await cancelCurrentExchange();
    expect(getDraft('t-1').text).toBe('first thread');
    expect(getDraft('t-2').text).toBe('second thread');
  });

  it('releases the canceling flag when inputs were taken back without a live turn', async () => {
    installCancel([{ id: 'q1', text: 'unstarted', image_hashes: [] }], [], false);
    await handleCancelExchange('t-1');
    expect(cancelingThreadIds.value.has('t-1')).toBe(false);
    expect(refreshThreadEvents).toHaveBeenCalledWith('t-1');
    expect(getDraft('t-1').text).toBe('unstarted');
  });
  it.each([
    { name: 'malformed JSON', response: () => new Response('{') },
    { name: 'an interrupted response body', response: () => new Response(new ReadableStream({
      start(controller) { controller.error(new TypeError('Load failed')); },
    })) },
  ])('surfaces $name instead of silently losing taken-back content', async ({ response }) => {
    globalThis.fetch = vi.fn(async () => response()) as unknown as typeof fetch;
    expect(await cancelCurrentExchange('t-1')).toBe('failed');
    expect(toasts.value.some(toast => toast.type === 'error')).toBe(true);
    expect(globalThis.fetch).toHaveBeenCalledTimes(1);
  });

});
