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

import { discardCompose, ensureFocusedComposeThread, sendFollowup, updateCompose, applyRemoteCompose } from './compose';
import { focusThread, unfocusThread } from './threads';
import { connectionStatus, focusedThreadId, threadMap, FOCUSED_THREAD_KEY } from '../store';
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
      ccHasChanges: false,
      ccRequiresRestart: false,
      ccIsExternalRepo: false,
      ccApplying: false,
      lastRevivedAt: '',
      messageCount: 0,
      section: 'archived',
      activeChildrenCount: 0,
      totalChildrenCount: 0,
      state: 'active',
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
