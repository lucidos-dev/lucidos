/**
 * Without compose-clear on the active-thread send path, the textarea's
 * useEffect resyncs el.value from the draft signal after submit and the
 * "Discard draft" button stays visible — even though the message was already
 * delivered. The bug surfaced most visibly when a user typed a free-text
 * answer to an AskUserQuestion: the answer rendered as "YOUR ANSWER" in the
 * exchange but the same text persisted as a Draft below.
 */
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
  fetchThreadEvents: vi.fn().mockResolvedValue([]),
}));

import { discardCompose, ensureFocusedComposeThread, pendingComposePuts, sendCompose, sendFollowup, updateCompose, applyRemoteCompose } from './compose';
import { focusThread, unfocusThread } from './threads';
import { connectionStatus, focusedThreadId, inputMode, threadMap, FOCUSED_THREAD_KEY, toasts } from '../store';
import {
  _resetThreadNavForTesting,
  _threadNavStateForTesting,
  pushThreadNavState,
  threadNavBack,
  threadNavForward,
  canGoBackThread,
  canGoForwardThread,
} from './thread-navigation';
import type { ThreadMeta, ThreadState } from '../thread-events';
import { _resetComposeDraftsForTesting, getDraft, setDraft, type ComposeDraft } from '../composeDrafts';

const originalFetch = globalThis.fetch;

interface MakeThreadOpts extends Partial<ThreadMeta> {
  composeText?: string;
  composeImages?: string[];
  composeMode?: ComposeDraft['mode'];
}

function makeThread(overrides: MakeThreadOpts = {}): ThreadState {
  const { composeText, composeImages, composeMode, ...metaOverrides } = overrides;
  const id = metaOverrides.id ?? 't-1';
  if (composeText !== undefined || composeImages !== undefined || composeMode !== undefined) {
    setDraft(id, {
      text: composeText ?? '',
      image_hashes: composeImages ?? [],
      mode: composeMode ?? null,
    });
  }
  return {
    meta: {
      id,
      title: '',
      channel: 'chat',
      initiator: 'user',
      saved: false,
      createdAt: '',
      updatedAt: '',
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
      ...metaOverrides,
    },
    events: new Map(),
    streamingBuffer: '',
    eventsLoaded: true,
    eventsLoadFailed: false,
    lastDbSeq: 0,
    pendingUserMessages: [],
  };
}

function makeActiveThread(overrides: MakeThreadOpts = {}): ThreadState {
  return makeThread({ state: 'active', ...overrides });
}

describe('sendFollowup clears the active-thread draft', () => {
  let mockFetch: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    mockFetch = vi.fn().mockResolvedValue(new Response(null, { status: 200 }));
    globalThis.fetch = mockFetch as unknown as typeof fetch;
    connectionStatus.value = 'connected';
    focusedThreadId.value = 't-1';
    const map = new Map<string, ThreadState>();
    map.set('t-1', makeActiveThread({ composeText: 'my answer', composeImages: ['iVBORfake'] }));
    threadMap.value = map;
  });

  afterEach(() => {
    globalThis.fetch = originalFetch;
    connectionStatus.value = 'disconnected';
    focusedThreadId.value = null;
    threadMap.value = new Map();
    _resetComposeDraftsForTesting();
    vi.restoreAllMocks();
  });

  it('clears composeText and composeImages on the focused active thread', async () => {
    await sendFollowup('t-1', 'my answer', undefined, {});
    const draft = getDraft('t-1');
    expect(draft.text).toBe('');
    expect(draft.image_hashes).toEqual([]);
  });

  it('clears the draft optimistically — before sendMessage resolves', async () => {
    let resolveSend: ((value: Response) => void) | null = null;
    mockFetch.mockImplementation(() => new Promise<Response>((resolve) => { resolveSend = resolve; }));

    const sendPromise = sendFollowup('t-1', 'my answer', undefined, {});

    // sendMessage is in flight; the draft must already be cleared so the
    // textarea's useEffect doesn't re-render the typed text as a Draft.
    const draft = getDraft('t-1');
    expect(draft.text).toBe('');
    expect(draft.image_hashes).toEqual([]);

    resolveSend!(new Response(null, { status: 200 }));
    await sendPromise;
  });
});

/** A composing (new) draft with no text and no images is not a draft —
 *  it's a ghost row in the drawer with the placeholder title "Empty draft".
 *  The user clearing the input is a discard signal: collapse the row entirely.
 *  Active threads keep their compose fields cleared but stay visible (the
 *  conversation still exists). */
describe('updateCompose discards empty composing drafts', () => {
  let mockFetch: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    mockFetch = vi.fn().mockResolvedValue(new Response(null, { status: 200 }));
    globalThis.fetch = mockFetch as unknown as typeof fetch;
    connectionStatus.value = 'connected';
    focusedThreadId.value = 't-1';
  });

  afterEach(() => {
    globalThis.fetch = originalFetch;
    connectionStatus.value = 'disconnected';
    focusedThreadId.value = null;
    threadMap.value = new Map();
    _resetComposeDraftsForTesting();
    vi.restoreAllMocks();
  });

  it('marks a composing thread discarded when text is cleared and no images remain', () => {
    const map = new Map<string, ThreadState>();
    map.set('t-1', makeThread({ state: 'composing', composeText: 'hello' }));
    threadMap.value = map;

    updateCompose('t-1', { text: '' });

    expect(threadMap.value.get('t-1')!.meta.state).toBe('discarded');
    expect(focusedThreadId.value).toBeNull();
  });

  it('discards when only whitespace remains', () => {
    const map = new Map<string, ThreadState>();
    map.set('t-1', makeThread({ state: 'composing', composeText: 'hello' }));
    threadMap.value = map;

    updateCompose('t-1', { text: '   \n\t' });

    expect(threadMap.value.get('t-1')!.meta.state).toBe('discarded');
  });

  it('keeps a composing thread alive when images are still attached', () => {
    const map = new Map<string, ThreadState>();
    map.set('t-1', makeThread({ state: 'composing', composeText: 'hello', composeImages: ['iVBORfake'] }));
    threadMap.value = map;

    updateCompose('t-1', { text: '' });

    expect(threadMap.value.get('t-1')!.meta.state).toBe('composing');
    expect(getDraft('t-1').text).toBe('');
    expect(focusedThreadId.value).toBe('t-1');
  });

  it('does NOT discard active threads when their follow-up draft is cleared', () => {
    // The conversation continues to exist; only the unsent follow-up is empty.
    const map = new Map<string, ThreadState>();
    map.set('t-1', makeActiveThread({ composeText: 'follow-up' }));
    threadMap.value = map;

    updateCompose('t-1', { text: '' });

    expect(threadMap.value.get('t-1')!.meta.state).toBe('active');
    expect(getDraft('t-1').text).toBe('');
    expect(focusedThreadId.value).toBe('t-1');
  });

  it('discards when the last image is removed and text is already empty', () => {
    const map = new Map<string, ThreadState>();
    map.set('t-1', makeThread({ state: 'composing', composeText: '', composeImages: ['iVBORfake'] }));
    threadMap.value = map;

    updateCompose('t-1', { image_hashes: [] });

    expect(threadMap.value.get('t-1')!.meta.state).toBe('discarded');
  });

  it('clears focusedThreadId only when the discarded thread was the focused one', () => {
    const map = new Map<string, ThreadState>();
    map.set('t-1', makeThread({ state: 'composing', composeText: 'hello' }));
    map.set('t-2', makeThread({ state: 'composing', composeText: 'other' }));
    threadMap.value = map;
    focusedThreadId.value = 't-2';

    updateCompose('t-1', { text: '' });

    expect(threadMap.value.get('t-1')!.meta.state).toBe('discarded');
    expect(focusedThreadId.value).toBe('t-2');
  });

  /** Regression: clicking the actor toggle (Lucidos/Claude) on an empty
   *  focused composing thread used to trigger the auto-discard, which then
   *  reset inputMode back to lucidos via discardCompose's session-stickiness
   *  guard. Net effect: user clicks Claude, toggle snaps back to Lucidos,
   *  user has to click 2-3 times before CC actually sticks. Mode-only patches
   *  are a "I'm preparing to type, in this channel" signal — NOT a content
   *  clear — so they must not trigger the ghost-row cleanup. */
  it('does NOT auto-discard a mode-only patch on an empty composing draft (regression: actor-toggle-bounces-back-to-lucidos)', () => {
    const map = new Map<string, ThreadState>();
    map.set('t-1', makeThread({ state: 'composing', composeText: '', composeImages: [], composeMode: null }));
    threadMap.value = map;
    focusedThreadId.value = 't-1';
    inputMode.value = { type: 'claude_code' };

    updateCompose('t-1', { mode: 'claude_code' });

    expect(threadMap.value.get('t-1')!.meta.state).toBe('composing');
    expect(focusedThreadId.value).toBe('t-1');
    expect(getDraft('t-1').mode).toBe('claude_code');
    // The discardCompose path would have reset this to {type:'do'} as a
    // session-stickiness guard — proving auto-discard fired.
    expect(inputMode.value).toEqual({ type: 'claude_code' });
  });

  it('still auto-discards when text is cleared in the same patch that sets a mode', () => {
    // User typed something, then in one combined update both cleared the text
    // AND switched mode (unlikely from the UI but defensible at the API layer).
    // The text clear is a real discard signal; the mode side ride-along loses.
    const map = new Map<string, ThreadState>();
    map.set('t-1', makeThread({ state: 'composing', composeText: 'hello', composeMode: 'lucidos' }));
    threadMap.value = map;

    updateCompose('t-1', { text: '', mode: 'claude_code' });

    expect(threadMap.value.get('t-1')!.meta.state).toBe('discarded');
  });
});

describe('draft threads land in the navigation history', () => {
  let mockFetch: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    mockFetch = vi.fn().mockResolvedValue(new Response(null, { status: 200 }));
    globalThis.fetch = mockFetch as unknown as typeof fetch;
    connectionStatus.value = 'connected';
    focusedThreadId.value = null;
    threadMap.value = new Map();
    _resetThreadNavForTesting();
  });

  afterEach(() => {
    globalThis.fetch = originalFetch;
    connectionStatus.value = 'disconnected';
    focusedThreadId.value = null;
    threadMap.value = new Map();
    _resetThreadNavForTesting();
    vi.restoreAllMocks();
  });

  it('focusThread → unfocusThread → ensureFocusedComposeThread: back/forward walks X ↔ draft', () => {
    focusThread('X');
    unfocusThread();
    const draftId = ensureFocusedComposeThread();

    expect(_threadNavStateForTesting().stack).toEqual([
      { type: 'thread', id: 'X' },
      { type: 'thread', id: draftId },
    ]);
    expect(_threadNavStateForTesting().cursor).toBe(1);

    expect(canGoBackThread.value).toBe(true);
    expect(canGoForwardThread.value).toBe(false);

    threadNavBack();
    expect(focusedThreadId.value).toBe('X');
    expect(_threadNavStateForTesting().cursor).toBe(0);
    expect(canGoForwardThread.value).toBe(true);

    threadNavForward();
    expect(focusedThreadId.value).toBe(draftId);
    expect(_threadNavStateForTesting().cursor).toBe(1);
  });

  it('does not push when typing into an already-focused thread', () => {
    // Already-focused threads (a follow-up draft on an active conversation,
    // or a thread the user just opened) are already in the nav stack via
    // focusThread; ensureFocusedComposeThread must be idempotent here.
    pushThreadNavState({ type: 'thread', id: 'active' });
    focusedThreadId.value = 'active';
    const before = _threadNavStateForTesting();
    const id = ensureFocusedComposeThread();
    expect(id).toBe('active');
    const after = _threadNavStateForTesting();
    expect(after.stack).toEqual(before.stack);
    expect(after.cursor).toBe(before.cursor);
  });

  it('discarding the focused draft removes it from the nav stack', async () => {
    pushThreadNavState({ type: 'thread', id: 'prior-thread' });
    const draftId = ensureFocusedComposeThread();
    threadMap.value = new Map(threadMap.value).set(draftId, makeThread({
      id: draftId,
      state: 'composing',
      latestTodoList: null,
    }));

    await discardCompose(draftId);

    const { stack, cursor } = _threadNavStateForTesting();
    expect(stack).toEqual([{ type: 'thread', id: 'prior-thread' }]);
    expect(cursor).toBe(0);
  });

  it('Back after Compose+type+Discard returns to the originally focused thread', async () => {
    // Regression: cursor lands on X after the draft is removed, but
    // focusedThreadId is null (the user is in the compose pane). Plain
    // "decrement cursor" then skips X — the user lands on the entry before X
    // even though they expected to return to X.
    focusThread('Y');
    focusThread('X');
    expect(focusedThreadId.value).toBe('X');

    unfocusThread();
    const draftId = ensureFocusedComposeThread();
    threadMap.value = new Map(threadMap.value).set(draftId, makeThread({
      id: draftId,
      state: 'composing',
      latestTodoList: null,
    }));

    await discardCompose(draftId);

    expect(focusedThreadId.value).toBeNull();
    expect(_threadNavStateForTesting().stack).toEqual([
      { type: 'thread', id: 'Y' },
      { type: 'thread', id: 'X' },
    ]);
    expect(_threadNavStateForTesting().cursor).toBe(1);

    threadNavBack();

    expect(focusedThreadId.value).toBe('X');
    expect(_threadNavStateForTesting().cursor).toBe(1);

    threadNavBack();

    expect(focusedThreadId.value).toBe('Y');
    expect(_threadNavStateForTesting().cursor).toBe(0);
  });

  it('Back after Compose without typing also returns to the focused thread', () => {
    // Same root cause as the discard case: focusedThreadId is null while the
    // cursor still points at the thread the user just left. The first Back
    // press must land on it instead of skipping past it.
    focusThread('Y');
    focusThread('X');

    unfocusThread();
    expect(focusedThreadId.value).toBeNull();
    expect(_threadNavStateForTesting().cursor).toBe(1);

    expect(canGoBackThread.value).toBe(true);
    threadNavBack();

    expect(focusedThreadId.value).toBe('X');
    expect(_threadNavStateForTesting().cursor).toBe(1);
  });

  it('drops the nav entry when startComposeIfNeeded POST fails', async () => {
    // rollbackOptimistic deletes the failed thread from threadMap; without the
    // matching nav cleanup, Forward would later restore an id whose events
    // 404 because no map entry exists.
    pushThreadNavState({ type: 'thread', id: 'prior-thread' });
    mockFetch.mockRejectedValueOnce(new Error('network down'));

    const draftId = ensureFocusedComposeThread();
    expect(_threadNavStateForTesting().stack).toEqual([
      { type: 'thread', id: 'prior-thread' },
      { type: 'thread', id: draftId },
    ]);

    await new Promise((r) => setTimeout(r, 0));

    expect(_threadNavStateForTesting().stack).toEqual([{ type: 'thread', id: 'prior-thread' }]);
    expect(focusedThreadId.value).toBeNull();
  });

  it('clearing draft text auto-discards it AND drops it from the nav stack', () => {
    // updateCompose's empty-content branch routes through discardCompose,
    // so the nav cleanup must travel that path too — otherwise every "type
    // then erase" cycle leaves a phantom entry in the stack.
    pushThreadNavState({ type: 'thread', id: 'prior-thread' });
    const draftId = ensureFocusedComposeThread();
    threadMap.value = new Map(threadMap.value).set(draftId, makeThread({
      id: draftId,
      state: 'composing',
      latestTodoList: null,
    }));

    updateCompose(draftId, { text: '' });

    const { stack, cursor } = _threadNavStateForTesting();
    expect(stack).toEqual([{ type: 'thread', id: 'prior-thread' }]);
    expect(cursor).toBe(0);
  });
});

describe('compose mutations leave updatedAt alone', () => {
  // Without this guard a keystroke jumps the thread to the top of saved /
  // archive (sorted by updatedAt) and the row's timestamp creeps forward
  // even though no message was sent.
  beforeEach(() => {
    const map = new Map<string, ThreadState>();
    map.set('t-1', makeActiveThread({ updatedAt: '2026-05-01T00:00:00.000Z' }));
    threadMap.value = map;
    connectionStatus.value = 'connected';
    focusedThreadId.value = 't-1';
  });

  afterEach(() => {
    connectionStatus.value = 'disconnected';
    focusedThreadId.value = null;
    threadMap.value = new Map();
  });

  it('updateCompose does not bump updatedAt', () => {
    updateCompose('t-1', { text: 'typing...' });
    expect(threadMap.value.get('t-1')!.meta.updatedAt).toBe('2026-05-01T00:00:00.000Z');
  });

  it('applyRemoteCompose does not bump updatedAt', () => {
    applyRemoteCompose('t-1', {
      text: 'from peer device',
      image_hashes: [],
      mode: null,
    });
    expect(threadMap.value.get('t-1')!.meta.updatedAt).toBe('2026-05-01T00:00:00.000Z');
  });
});

/** Draft mutations must not invalidate threadMap. ChatExchange subscribes
 *  to threadMap and runs marked.parse per render, so a per-keystroke
 *  threadMap write would re-parse every exchange in the thread. */
describe('updateCompose does not mutate threadMap (perf isolation)', () => {
  beforeEach(() => {
    const map = new Map<string, ThreadState>();
    map.set('t-1', makeActiveThread());
    threadMap.value = map;
    connectionStatus.value = 'connected';
    focusedThreadId.value = 't-1';
  });

  afterEach(() => {
    connectionStatus.value = 'disconnected';
    focusedThreadId.value = null;
    threadMap.value = new Map();
  });

  it('keystroke updates do not reassign threadMap.value', () => {
    const before = threadMap.value;
    updateCompose('t-1', { text: 'a' });
    updateCompose('t-1', { text: 'ab' });
    updateCompose('t-1', { text: 'abc' });
    expect(threadMap.value).toBe(before);
  });

  it('keystroke updates do not produce a new ThreadState entry', () => {
    const before = threadMap.value.get('t-1');
    updateCompose('t-1', { text: 'typing' });
    expect(threadMap.value.get('t-1')).toBe(before);
  });

  it('applyRemoteCompose (peer SSE) also leaves threadMap untouched', () => {
    const before = threadMap.value;
    applyRemoteCompose('t-1', { text: 'from peer', image_hashes: [], mode: null });
    expect(threadMap.value).toBe(before);
  });
});

/** Compose drafts are allocated client-side and live as `composing` rows
 *  server-side. The thread id never appears in the URL — it lives only on the
 *  focusedThreadId signal and (via setFocusedThread) FOCUSED_THREAD_KEY in
 *  localStorage. Without the localStorage write, reloading mid-draft loses
 *  the id and the next keystroke allocates a fresh UUID, orphaning the
 *  server-side compose row from the previous session. */
describe('ensureFocusedComposeThread persists focusedThreadId across reload', () => {
  let mockFetch: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    mockFetch = vi.fn().mockResolvedValue(new Response(null, { status: 200 }));
    globalThis.fetch = mockFetch as unknown as typeof fetch;
    connectionStatus.value = 'connected';
    focusedThreadId.value = null;
    threadMap.value = new Map();
    localStorage.removeItem(FOCUSED_THREAD_KEY);
    _resetThreadNavForTesting();
  });

  afterEach(() => {
    globalThis.fetch = originalFetch;
    connectionStatus.value = 'disconnected';
    focusedThreadId.value = null;
    threadMap.value = new Map();
    localStorage.removeItem(FOCUSED_THREAD_KEY);
    _resetThreadNavForTesting();
    vi.restoreAllMocks();
  });

  it('writes the allocated draft id to localStorage so the next reload resumes the same draft', () => {
    const id = ensureFocusedComposeThread();
    expect(localStorage.getItem(FOCUSED_THREAD_KEY)).toBe(id);
  });

  it('clears localStorage when discardCompose nulls the focused draft', async () => {
    const id = ensureFocusedComposeThread();
    threadMap.value = new Map(threadMap.value).set(id, makeThread({
      id,
      state: 'composing',
      composeText: 'hi',
    }));
    expect(localStorage.getItem(FOCUSED_THREAD_KEY)).toBe(id);

    await discardCompose(id);

    expect(focusedThreadId.value).toBeNull();
    expect(localStorage.getItem(FOCUSED_THREAD_KEY)).toBeNull();
  });

  it('clears localStorage on rollback when startComposeIfNeeded POST fails', async () => {
    mockFetch.mockRejectedValueOnce(new Error('network down'));
    const id = ensureFocusedComposeThread();
    expect(localStorage.getItem(FOCUSED_THREAD_KEY)).toBe(id);

    await new Promise((r) => setTimeout(r, 0));

    expect(focusedThreadId.value).toBeNull();
    expect(localStorage.getItem(FOCUSED_THREAD_KEY)).toBeNull();
  });

  it('does NOT touch localStorage when called with a thread already focused', () => {
    // Already-focused threads went through focusThread / setFocusedThread,
    // which already wrote the id; ensureFocusedComposeThread is a no-op.
    focusThread('existing');
    expect(localStorage.getItem(FOCUSED_THREAD_KEY)).toBe('existing');

    const id = ensureFocusedComposeThread();
    expect(id).toBe('existing');
    expect(localStorage.getItem(FOCUSED_THREAD_KEY)).toBe('existing');
  });
});

/** Race regression: ensureFocusedComposeThread fires POST /threads as
 *  fire-and-forget. The compose PUT is debounced ~250ms. On a slow link the
 *  POST hasn't landed when the debounce fires, so the PUT 404s with
 *  "thread not found" and the user sees one error toast per typing burst.
 *  pushNow must await the in-flight thread-start before issuing the PUT. */
describe('compose PUT waits for the thread-start POST to land (slow-network race)', () => {
  beforeEach(() => {
    connectionStatus.value = 'connected';
    focusedThreadId.value = null;
    threadMap.value = new Map();
    _resetComposeDraftsForTesting();
    _resetThreadNavForTesting();
    localStorage.removeItem(FOCUSED_THREAD_KEY);
    toasts.value = [];
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
    globalThis.fetch = originalFetch;
    connectionStatus.value = 'disconnected';
    focusedThreadId.value = null;
    threadMap.value = new Map();
    _resetComposeDraftsForTesting();
    _resetThreadNavForTesting();
    localStorage.removeItem(FOCUSED_THREAD_KEY);
    toasts.value = [];
    vi.restoreAllMocks();
  });

  it('debounced PUT does not fire until POST /threads resolves; no 404 toast', async () => {
    const seen: Array<{ method: string; url: string }> = [];
    let resolveCreate: ((r: Response) => void) | null = null;

    const mockFetch = vi.fn((url: string, init?: RequestInit) => {
      const method = (init?.method ?? 'GET').toUpperCase();
      seen.push({ method, url });
      if (url === '/api/v1/threads' && method === 'POST') {
        return new Promise<Response>((r) => { resolveCreate = r; });
      }
      if (url.endsWith('/compose') && method === 'PUT') {
        return Promise.resolve(new Response(null, { status: 204 }));
      }
      return Promise.resolve(new Response(null, { status: 200 }));
    });
    globalThis.fetch = mockFetch as unknown as typeof fetch;

    const isComposePut = (s: { method: string; url: string }) =>
      s.method === 'PUT' && s.url.endsWith('/compose');

    // Simulate first keystroke: allocate the thread + schedule the PUT.
    const id = ensureFocusedComposeThread();
    updateCompose(id, { text: 'D' });

    // Walk past the 250ms debounce window. The buggy version fires the PUT
    // here while POST /threads is still in flight → server 404 → toast.
    await vi.advanceTimersByTimeAsync(300);

    expect(
      seen.filter(isComposePut),
      'compose PUT must NOT fire while POST /threads is still in flight',
    ).toEqual([]);

    // Resolve POST /threads; the queued PUT should fire now.
    resolveCreate!(new Response(null, { status: 201 }));
    await vi.runAllTimersAsync();

    expect(seen.filter(isComposePut)).toHaveLength(1);
    expect(toasts.value.filter((t) => t.type === 'error')).toEqual([]);
  });

  it('clears pendingComposePuts and suppresses duplicate toast when POST /threads rejects', async () => {
    // ensureFocusedComposeThread already toasts "Failed to start compose"
    // when its POST rejects. Without try/finally + an inner catch, pushNow
    // would (a) leak the pendingComposePuts entry, blocking subsequent
    // loadAllThreads refreshes for that thread for the rest of the session,
    // and (b) toast a second "Compose sync failed" with the same error.
    let rejectCreate: ((err: Error) => void) | null = null;
    const mockFetch = vi.fn((url: string, init?: RequestInit) => {
      const method = (init?.method ?? 'GET').toUpperCase();
      if (url === '/api/v1/threads' && method === 'POST') {
        return new Promise<Response>((_, reject) => { rejectCreate = reject; });
      }
      return Promise.resolve(new Response(null, { status: 200 }));
    });
    globalThis.fetch = mockFetch as unknown as typeof fetch;

    const id = ensureFocusedComposeThread();
    updateCompose(id, { text: 'hello' });
    expect(pendingComposePuts.has(id)).toBe(true);

    await vi.advanceTimersByTimeAsync(300);

    rejectCreate!(new Error('network down'));
    await vi.runAllTimersAsync();

    expect(pendingComposePuts.has(id)).toBe(false);
    const errorToasts = toasts.value.filter((t) => t.type === 'error');
    expect(errorToasts).toHaveLength(1);
    expect(errorToasts[0].message).toMatch(/Failed to start compose/);
  });

  it('skips PUT when discardCompose runs while POST /threads is still in flight', async () => {
    // Pre-fix, the await widened the discard race: discardCompose flips state
    // to 'discarded' (thread stays in threadMap), pushNow's `if (!thread)`
    // doesn't catch it, so when POST resolves the PUT fires against a
    // discarded thread → backend 410 → toast "Compose sync failed: thread
    // discarded". The post-await guard must reject discarded state too.
    const seen: Array<{ method: string; url: string }> = [];
    let resolveCreate: ((r: Response) => void) | null = null;
    const mockFetch = vi.fn((url: string, init?: RequestInit) => {
      const method = (init?.method ?? 'GET').toUpperCase();
      seen.push({ method, url });
      if (url === '/api/v1/threads' && method === 'POST') {
        return new Promise<Response>((r) => { resolveCreate = r; });
      }
      return Promise.resolve(new Response(null, { status: 204 }));
    });
    globalThis.fetch = mockFetch as unknown as typeof fetch;

    const id = ensureFocusedComposeThread();
    updateCompose(id, { text: 'about to discard' });

    await vi.advanceTimersByTimeAsync(300);

    // User discards before POST resolves. discardCompose flips state to
    // 'discarded' and DELETEs server-side; the in-flight pushNow must skip.
    void discardCompose(id);

    resolveCreate!(new Response(null, { status: 201 }));
    await vi.runAllTimersAsync();

    expect(
      seen.filter((s) => s.method === 'PUT' && s.url.endsWith('/compose')),
      'no compose PUT should fire against a discarded thread',
    ).toEqual([]);
    expect(toasts.value.filter((t) => t.type === 'error')).toEqual([]);
  });
});

/** The user-visible contract: picking Claude (or Lucidos) sticks. send /
 *  discard MUST NOT reset inputMode — the next fresh compose has to start on
 *  whichever channel the user last picked, both visually (toggle display) and
 *  functionally (send routing). Cross-reload persistence is covered by
 *  `store/inputMode-default.test.ts`. */
describe('inputMode is sticky across compose sessions (toggle remembers last pick)', () => {
  let mockFetch: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    mockFetch = vi.fn().mockResolvedValue(new Response(null, { status: 200 }));
    globalThis.fetch = mockFetch as unknown as typeof fetch;
    connectionStatus.value = 'connected';
    focusedThreadId.value = 't-1';
    inputMode.value = { type: 'claude_code' };
  });

  afterEach(() => {
    globalThis.fetch = originalFetch;
    connectionStatus.value = 'disconnected';
    focusedThreadId.value = null;
    threadMap.value = new Map();
    inputMode.value = { type: 'do' };
    _resetComposeDraftsForTesting();
    vi.restoreAllMocks();
  });

  it('sendCompose leaves inputMode alone so the next fresh compose stays on the user pick', async () => {
    const map = new Map<string, ThreadState>();
    map.set('t-1', makeThread({
      state: 'composing',
      channel: 'claude_code',
      composeText: 'fix the bug',
      composeMode: 'claude_code',
    }));
    threadMap.value = map;

    await sendCompose('t-1', { useClaudeCode: true });

    expect(inputMode.value).toEqual({ type: 'claude_code' });
  });

  it('discardCompose leaves inputMode alone so the next fresh compose stays on the user pick', async () => {
    const map = new Map<string, ThreadState>();
    map.set('t-1', makeThread({
      state: 'composing',
      channel: 'claude_code',
      composeText: 'never mind',
      composeMode: 'claude_code',
    }));
    threadMap.value = map;

    await discardCompose('t-1');

    expect(inputMode.value).toEqual({ type: 'claude_code' });
  });

  it('sendCompose started from lucidos leaves inputMode at lucidos', async () => {
    inputMode.value = { type: 'do' };
    const map = new Map<string, ThreadState>();
    map.set('t-1', makeThread({
      state: 'composing',
      channel: 'chat',
      composeText: 'hello',
      composeMode: 'lucidos',
    }));
    threadMap.value = map;

    await sendCompose('t-1', { useClaudeCode: false });

    expect(inputMode.value).toEqual({ type: 'do' });
  });
});
