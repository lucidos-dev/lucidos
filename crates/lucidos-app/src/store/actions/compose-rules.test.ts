import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';

vi.hoisted(() => {
  if (typeof globalThis.document === 'undefined') (globalThis as any).document = {};
  const doc = globalThis.document as any;
  if (!doc.querySelector) doc.querySelector = () => null;
  if (!doc.querySelectorAll) doc.querySelectorAll = () => [];
  if (typeof globalThis.requestAnimationFrame === 'undefined') {
    (globalThis as any).requestAnimationFrame = (cb: any) => { cb(); return 0; };
  }
});

vi.mock('../../api/threads', () => ({
  fetchThreadEvents: vi.fn().mockResolvedValue({ events: [], currentAggregate: null }),
}));

import {
  applyDestination,
  discardCompose,
  ensureFocusedComposeThread,
  seedSuggestion,
  sendCompose,
  sendSeededPrompt,
  updateCompose,
  _resetUndeliveredComposeDraftsForTesting,
} from './compose';
import { _resetRowIntentsForTesting } from '../rowIntents';
import { acceptComposeReport, isComposeIntentPending, _resetComposeSyncForTesting } from '../composeSync';
import { connectionStatus, focusedPane, focusedThreadId, inputMode, threadMap, toasts } from '../store';
import { _resetComposeDraftsForTesting, getDraft, setDraft } from '../composeDrafts';
import { resolveScope, _resetComposeSelectionsForTesting } from '../composeSelections';
import {
  getSessionBlobUrlForHash,
  rememberSessionBlobUrl,
  _resetSessionBlobUrlsForTesting,
} from '../../components/chat/pastedImages';
import type { ThreadMeta, ThreadState } from '../thread-events';

const originalFetch = globalThis.fetch;

function makeThread(id: string, state: ThreadMeta['state'], channel: ThreadMeta['channel'] = 'chat'): ThreadState {
  return {
    meta: {
      id,
      title: '',
      channel,
      initiator: 'user',
      saved: false,
      createdAt: '',
      updatedAt: '',
      status: 'idle',
      summaryVersion: 0,
      codingAgentChangeState: { kind: 'none' },
      codingAgentIsExternalRepo: false,
      lastRevivedAt: '',
      messageCount: 0,
      section: 'archived',
      activeChildrenCount: 0,
      totalChildrenCount: 0,
      blockingDescendantCount: 0,
      attentionDescendantCount: 0,
      archivableSubThreadCount: 0,
      state,
      latestTodoList: null,
      liveEventWaitCount: 0,
      liveEventWaits: [],
    },
    events: new Map(),
    streamingBuffer: '',
    eventsLoaded: true,
    eventsLoadFailed: false,
    lastDbSeq: 0,
    pendingUserMessages: [],
  };
}

const okEmpty = () => Promise.resolve(new Response(null, { status: 204 }));

describe('compose rules enforced by tests', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    connectionStatus.value = 'connected';
    focusedThreadId.value = null;
    focusedPane.value = 'thread';
    inputMode.value = { type: 'do' };
    threadMap.value = new Map();
    toasts.value = [];
    _resetComposeDraftsForTesting();
    _resetUndeliveredComposeDraftsForTesting();
    _resetComposeSyncForTesting();
    _resetComposeSelectionsForTesting();
    _resetSessionBlobUrlsForTesting();
  });

  afterEach(() => {
    vi.useRealTimers();
    globalThis.fetch = originalFetch;
    connectionStatus.value = 'disconnected';
    focusedThreadId.value = null;
    threadMap.value = new Map();
    toasts.value = [];
    _resetComposeDraftsForTesting();
    _resetUndeliveredComposeDraftsForTesting();
    _resetComposeSyncForTesting();
    _resetComposeSelectionsForTesting();
    _resetSessionBlobUrlsForTesting();
    _resetRowIntentsForTesting();
    vi.restoreAllMocks();
  });

  it('a write answered after its draft was discarded does not bring the draft back', async () => {
    let answerPut!: (response: Response) => void;
    let answerDelete!: (response: Response) => void;
    globalThis.fetch = vi.fn((_url: unknown, init?: RequestInit) => {
      if (init?.method === 'PUT') return new Promise<Response>((resolve) => { answerPut = resolve; });
      if (init?.method === 'DELETE') return new Promise<Response>((resolve) => { answerDelete = resolve; });
      return okEmpty();
    }) as unknown as typeof fetch;
    threadMap.value = new Map([['t-1', makeThread('t-1', 'composing')]]);

    updateCompose('t-1', { text: 'typed' });
    await vi.advanceTimersByTimeAsync(300);
    const discarding = discardCompose('t-1');
    answerPut(new Response(null, { status: 204 }));
    await vi.advanceTimersByTimeAsync(0);
    expect(getDraft('t-1').text).toBe('');
    expect(isComposeIntentPending('t-1')).toBe(true);
    answerDelete(new Response(null, { status: 204 }));
    await discarding;

    expect(getDraft('t-1').text).toBe('');
  });

  it('a failed discard after a cleared draft restores the cleared text, not the text it replaced', async () => {
    globalThis.fetch = vi.fn((_url: unknown, init?: RequestInit) => (
      init?.method === 'DELETE'
        ? Promise.resolve(new Response(JSON.stringify({ error: 'boom' }), { status: 500 }))
        : okEmpty()
    )) as unknown as typeof fetch;
    threadMap.value = new Map([['t-1', makeThread('t-1', 'composing')]]);
    setDraft('t-1', { text: 'typed', image_hashes: [], mode: 'lucidos' });

    updateCompose('t-1', { text: '' });
    await vi.runAllTimersAsync();

    expect(getDraft('t-1').text).toBe('');
  });

  it('a scope pick on a draft is a pending write that a server report cannot revert', () => {
    globalThis.fetch = vi.fn(() => okEmpty()) as unknown as typeof fetch;
    threadMap.value = new Map([['d-1', makeThread('d-1', 'composing')]]);
    focusedThreadId.value = 'd-1';
    setDraft('d-1', { text: '', image_hashes: [], mode: 'claude_code' });

    applyDestination('d-1', { kind: 'coding', scope: { kind: 'external', repoId: 'repo-1' } });
    acceptComposeReport('d-1', { revision: 10, epoch: null, text: '', imageHashes: [], mode: null, selection: null });

    expect(resolveScope('d-1')).toEqual({ kind: 'external', repoId: 'repo-1' });
  });

  it('a seeded suggestion is born on the chat channel even from a coding-agent composer', () => {
    globalThis.fetch = vi.fn(() => okEmpty()) as unknown as typeof fetch;
    inputMode.value = { type: 'coding_agent' };

    const id = seedSuggestion('Where can I download apps?');

    expect(threadMap.value.get(id)?.meta.channel).toBe('chat');
  });

  it('seeding off an open thread pulls the thread pane forward when the content pane had focus', () => {
    globalThis.fetch = vi.fn(() => okEmpty()) as unknown as typeof fetch;
    threadMap.value = new Map([['a-1', makeThread('a-1', 'active')]]);
    focusedThreadId.value = 'a-1';
    focusedPane.value = 'content';

    seedSuggestion('Where can I download apps?');

    expect(focusedPane.value).toBe('thread');
  });

  it('starting a draft leaves the content pane in front', () => {
    globalThis.fetch = vi.fn(() => okEmpty()) as unknown as typeof fetch;
    focusedPane.value = 'content';

    ensureFocusedComposeThread();

    expect(focusedPane.value).toBe('content');
  });

  it('a seeded prompt whose send fails names the send that failed', async () => {
    globalThis.fetch = vi.fn(() => Promise.reject(new TypeError('Failed to fetch'))) as unknown as typeof fetch;

    const sent = await sendSeededPrompt('Where can I download apps?', 'start the chat');

    expect(sent).toBe(false);
    expect(toasts.value.map((t) => t.message)).toContain('Failed to start the chat: Failed to fetch');
  });

  it('a report from before a first send cannot restore the sent text while the send is in flight', () => {
    globalThis.fetch = vi.fn(() => new Promise<Response>(() => {})) as unknown as typeof fetch;
    threadMap.value = new Map([['t-1', makeThread('t-1', 'composing')]]);
    focusedThreadId.value = 't-1';
    setDraft('t-1', { text: 'hello', image_hashes: [], mode: 'lucidos' });

    void sendCompose('t-1', { focus: false });
    acceptComposeReport('t-1', { revision: 50, epoch: null, text: 'hello', imageHashes: [], mode: 'lucidos', selection: null });

    expect(getDraft('t-1').text).toBe('');
  });

  it('a stale write answer cannot replace text typed while it was in flight', async () => {
    let answerFirst!: (response: Response) => void;
    const bodies: Array<{ text: string }> = [];
    let calls = 0;
    globalThis.fetch = vi.fn((_url: unknown, init?: RequestInit) => {
      calls += 1;
      bodies.push(JSON.parse(String(init?.body)));
      if (calls === 1) return new Promise<Response>((resolve) => { answerFirst = resolve; });
      return okEmpty();
    }) as unknown as typeof fetch;
    threadMap.value = new Map([['t-1', makeThread('t-1', 'active')]]);
    focusedThreadId.value = 't-1';

    updateCompose('t-1', { text: 'first' });
    await vi.advanceTimersByTimeAsync(300);
    updateCompose('t-1', { text: 'second typed' });
    answerFirst(new Response(JSON.stringify({ compose_epoch: 7, text: 'server-old', image_hashes: [] }), { status: 412 }));
    await vi.runAllTimersAsync();

    expect(getDraft('t-1').text).toBe('second typed');
    expect(bodies[bodies.length - 1].text).toBe('second typed');
  });

  it('a write the engine refused keeps the typed text against a later report', async () => {
    globalThis.fetch = vi.fn(() => Promise.resolve(
      new Response(JSON.stringify({ error: 'bad body' }), { status: 400 }),
    )) as unknown as typeof fetch;
    threadMap.value = new Map([['t-1', makeThread('t-1', 'active')]]);
    focusedThreadId.value = 't-1';

    updateCompose('t-1', { text: 'typed' });
    await vi.runAllTimersAsync();
    acceptComposeReport('t-1', { revision: 99, epoch: null, text: 'server', imageHashes: [], mode: null, selection: null });

    expect(getDraft('t-1').text).toBe('typed');
  });

  it('a sent image keeps its preview while the send is in flight', () => {
    globalThis.fetch = vi.fn(() => new Promise<Response>(() => {})) as unknown as typeof fetch;
    rememberSessionBlobUrl('h1', 'blob:sent-image');
    threadMap.value = new Map([['t-1', makeThread('t-1', 'composing')]]);
    focusedThreadId.value = 't-1';
    setDraft('t-1', { text: 'look', image_hashes: ['h1'], mode: 'lucidos' });

    void sendCompose('t-1', { focus: false });

    expect(getSessionBlobUrlForHash('h1')).toBe('blob:sent-image');
  });
});
