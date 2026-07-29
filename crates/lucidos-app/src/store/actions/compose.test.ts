/**
 * Without compose-clear on the active-thread send path, the textarea's
 * useEffect resyncs el.value from the draft signal after submit and the
 * cleared draft reappears (the clear-X stays visible, the row lingers in the
 * Drafts panel) — even though the message was already delivered. The bug
 * surfaced most visibly when a user typed a free-text answer to an
 * AskUserQuestion: the answer rendered as "YOUR ANSWER" in the exchange but the
 * same text persisted as a Draft below.
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

import { applySuggestion, composeEditedAt, discardCompose, ensureFocusedComposeThread, pendingComposePuts, prefillCompose, sendCompose, sendFollowup, updateCompose, applyRemoteCompose } from './compose';
import { focusThread, unfocusThread } from './threads';
import { connectionStatus, confirmState, focusedThreadId, inputMode, threadMap, selectedScope, FOCUSED_THREAD_KEY, toasts } from '../store';
import { promptOverrideSyncSeq } from '../../components/chat/promptValueSync';
import { patchComposeSelection, getComposeSelectionOverride, resolveScope, _resetComposeSelectionsForTesting } from '../composeSelections';
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

  it('can send a queued follow-up to its original thread without stealing focus', async () => {
    const map = new Map<string, ThreadState>();
    map.set('t-1', makeActiveThread({ id: 't-1', composeText: 'queued', composeImages: ['hash-1'] }));
    map.set('t-2', makeActiveThread({ id: 't-2' }));
    threadMap.value = map;
    focusedThreadId.value = 't-2';

    await sendFollowup('t-1', 'queued', ['hash-1'], { focus: false });

    const chatCall = mockFetch.mock.calls.find(([url, init]) =>
      typeof url === 'string'
        && url.endsWith('/chat/stream')
        && (init as RequestInit | undefined)?.method === 'POST',
    );
    expect(chatCall, 'expected chat POST').toBeDefined();
    const body = JSON.parse((chatCall![1] as RequestInit).body as string);
    expect(body.thread_id).toBe('t-1');
    expect(body.image_hashes).toEqual(['hash-1']);
    expect(focusedThreadId.value).toBe('t-2');
  });
});

describe('prefillCompose — starter suggestion drop-in', () => {
  let mockFetch: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    mockFetch = vi.fn().mockResolvedValue(new Response(null, { status: 200 }));
    globalThis.fetch = mockFetch as unknown as typeof fetch;
    connectionStatus.value = 'connected';
  });

  afterEach(() => {
    globalThis.fetch = originalFetch;
    connectionStatus.value = 'disconnected';
    focusedThreadId.value = null;
    threadMap.value = new Map();
    _resetComposeDraftsForTesting();
    vi.restoreAllMocks();
  });

  it('writes the prompt into the focused composing draft and does NOT send', () => {
    const map = new Map<string, ThreadState>();
    map.set('t-1', makeThread({ state: 'composing' }));
    threadMap.value = map;
    focusedThreadId.value = 't-1';

    const id = prefillCompose('Build me an app that tracks my reading list.');

    expect(id).toBe('t-1');
    expect(getDraft('t-1').text).toBe('Build me an app that tracks my reading list.');
    // The thread stays composing — prefill is not a send.
    expect(threadMap.value.get('t-1')!.meta.state).toBe('composing');
    const chatCall = mockFetch.mock.calls.find(([url]) =>
      typeof url === 'string' && url.endsWith('/chat/stream'));
    expect(chatCall, 'prefill must not POST a chat message').toBeUndefined();
  });

  it('replaces the WHOLE input — clears attached images too, not just the text', () => {
    const map = new Map<string, ThreadState>();
    map.set('t-1', makeThread({ state: 'composing', composeText: 'old', composeImages: ['iVBORfake'] }));
    threadMap.value = map;
    focusedThreadId.value = 't-1';

    prefillCompose('a fresh starter prompt');

    expect(getDraft('t-1').text).toBe('a fresh starter prompt');
    // Stale attachments must not ride along with the unrelated starter.
    expect(getDraft('t-1').image_hashes).toEqual([]);
  });
});

describe('applySuggestion — welcome starter drop-in', () => {
  let mockFetch: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    mockFetch = vi.fn().mockResolvedValue(new Response(null, { status: 200 }));
    globalThis.fetch = mockFetch as unknown as typeof fetch;
    connectionStatus.value = 'connected';
    focusedThreadId.value = null;
    threadMap.value = new Map();
    inputMode.value = { type: 'do' };
    _resetComposeDraftsForTesting();
  });

  afterEach(() => {
    globalThis.fetch = originalFetch;
    connectionStatus.value = 'disconnected';
    focusedThreadId.value = null;
    threadMap.value = new Map();
    inputMode.value = { type: 'do' };
    confirmState.value = { visible: false, message: '', okLabel: 'Delete' };
    _resetComposeDraftsForTesting();
    vi.restoreAllMocks();
  });

  it('no draft in progress: drops the text in and does NOT confirm', async () => {
    const seqBefore = promptOverrideSyncSeq.value;

    const applied = await applySuggestion('Build me an app that tracks my reading list.');

    expect(applied).toBe(true);
    // No confirm was raised (no draft to protect).
    expect(confirmState.value.visible).toBe(false);
    const id = focusedThreadId.value!;
    expect(id).toBeTruthy();
    expect(getDraft(id).text).toBe('Build me an app that tracks my reading list.');
    // Force-sync ticket bumped so the textarea reflects the override.
    expect(promptOverrideSyncSeq.value).toBe(seqBefore + 1);
    // Not sent — prefill is not a send.
    const chatCall = mockFetch.mock.calls.find(([url]) =>
      typeof url === 'string' && url.endsWith('/chat/stream'));
    expect(chatCall, 'applySuggestion must not POST a chat message').toBeUndefined();
  });

  it('sets the destination to the Lucidos Agent (chat channel)', async () => {
    // Start on the coding-agent channel with a coding-agent composing draft.
    inputMode.value = { type: 'coding_agent' };
    const map = new Map<string, ThreadState>();
    map.set('cc-1', makeThread({ id: 'cc-1', state: 'composing', channel: 'claude_code', composeMode: 'claude_code' }));
    threadMap.value = map;
    focusedThreadId.value = 'cc-1';

    // Empty draft (mode-only) → no confirm.
    const applied = await applySuggestion('Tell me how to set up Lucidos for mobile access.');

    expect(applied).toBe(true);
    expect(inputMode.value).toEqual({ type: 'do' });
    expect(getDraft('cc-1').mode).toBe('lucidos');
    expect(getDraft('cc-1').text).toBe('Tell me how to set up Lucidos for mobile access.');
  });

  it('draft in progress + confirm accepted: overrides text AND clears attachments', async () => {
    const map = new Map<string, ThreadState>();
    map.set('t-1', makeThread({ state: 'composing', composeText: 'my own idea', composeImages: ['iVBORfake'] }));
    threadMap.value = map;
    focusedThreadId.value = 't-1';

    const p = applySuggestion('Where can I download apps?');
    // The confirm is raised synchronously (showConfirm's executor runs eagerly).
    expect(confirmState.value.visible).toBe(true);
    expect(confirmState.value.okLabel).toBe('Replace');
    confirmState.value.resolve!(true);

    expect(await p).toBe(true);
    expect(getDraft('t-1').text).toBe('Where can I download apps?');
    // "Replace" means the whole draft — stale attachments must not linger.
    expect(getDraft('t-1').image_hashes).toEqual([]);
  });

  it('draft in progress + confirm declined: keeps the draft untouched', async () => {
    const seqBefore = promptOverrideSyncSeq.value;
    const map = new Map<string, ThreadState>();
    map.set('t-1', makeThread({ state: 'composing', composeText: 'my own idea' }));
    threadMap.value = map;
    focusedThreadId.value = 't-1';

    const p = applySuggestion('Where can I download apps?');
    expect(confirmState.value.visible).toBe(true);
    confirmState.value.resolve!(false);

    expect(await p).toBe(false);
    // Draft untouched, no override sync fired.
    expect(getDraft('t-1').text).toBe('my own idea');
    expect(promptOverrideSyncSeq.value).toBe(seqBefore);
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
    inputMode.value = { type: 'coding_agent' };

    updateCompose('t-1', { mode: 'claude_code' });

    expect(threadMap.value.get('t-1')!.meta.state).toBe('composing');
    expect(focusedThreadId.value).toBe('t-1');
    expect(getDraft('t-1').mode).toBe('claude_code');
    // The discardCompose path would have reset this to {type:'do'} as a
    // session-stickiness guard — proving auto-discard fired.
    expect(inputMode.value).toEqual({ type: 'coding_agent' });
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

/** The SSE `ThreadComposeChanged` path (applyRemoteCompose) is the second of two
 *  places that apply a server compose snapshot to the local draft signal — the
 *  first being stageDraftFromApi (thread-loading.ts), guarded since the session-5
 *  drafts:65 fix. A remote EMPTY snapshot must never clear a non-empty draft this
 *  device authored: the only emitter is the compose PUT handler, and a PUT that
 *  fired before the device-id header was available broadcasts origin=None, which
 *  bypasses the SSE self-echo suppression (thread-sync.ts only suppresses when
 *  origin is present) and reaches applyRemoteCompose. Without the guard that empty
 *  echo blanks the just-typed draft — the value='' face of mobile-webkit
 *  drafts.spec.ts:65. See docs/plans/2026-06-28-drafts-sse-empty-clear-guard.md. */
describe('applyRemoteCompose empty-clear guard (drafts:65)', () => {
  beforeEach(() => {
    _resetComposeDraftsForTesting();
    composeEditedAt.delete('t-1');
    const map = new Map<string, ThreadState>();
    map.set('t-1', makeActiveThread());
    threadMap.value = map;
    connectionStatus.value = 'connected';
    focusedThreadId.value = 't-1';
  });

  afterEach(() => {
    composeEditedAt.delete('t-1');
    _resetComposeDraftsForTesting();
    connectionStatus.value = 'disconnected';
    focusedThreadId.value = null;
    threadMap.value = new Map();
  });

  it('INV1: a remote empty snapshot does not clear a locally-edited non-empty draft', () => {
    setDraft('t-1', { text: 'thread draft text', image_hashes: [], mode: null });
    composeEditedAt.set('t-1', Date.now());

    // The device's own / non-attributable empty echo arriving after the local edit.
    applyRemoteCompose('t-1', { text: '', image_hashes: [], mode: null });

    // Without the guard this reads '' (clearDraft fired) — the value='' flake.
    expect(getDraft('t-1').text).toBe('thread draft text');
  });

  it('INV2: a remote empty snapshot still clears a draft not edited on this device', () => {
    // Server-originated / peer-seeded draft: present locally but no local edit,
    // so composeEditedAt is intentionally absent for t-1.
    setDraft('t-1', { text: 'server seeded', image_hashes: [], mode: null });

    applyRemoteCompose('t-1', { text: '', image_hashes: [], mode: null });

    expect(getDraft('t-1').text).toBe('');
    expect(getDraft('t-1').image_hashes).toHaveLength(0);
  });

  it('INV3: a remote NON-empty edit still replaces even a locally-edited draft', () => {
    setDraft('t-1', { text: 'mine', image_hashes: [], mode: null });
    composeEditedAt.set('t-1', Date.now());

    // The guard skips only the EMPTY clear — a peer's non-empty edit still wins.
    applyRemoteCompose('t-1', { text: 'from peer', image_hashes: [], mode: null });

    expect(getDraft('t-1').text).toBe('from peer');
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

  it('eager-seeds the new draft\'s OWN scope from the last-used seed (selectedScope)', () => {
    // The new draft must carry its own scope because resolveScope no longer falls
    // back to selectedScope for a real draft (the no-leak guard). So a fresh draft
    // still shows the last-used target — via its own stored override, not the seed.
    selectedScope.value = { kind: 'app', appId: 'last-used-app' };
    const id = ensureFocusedComposeThread();
    expect(getComposeSelectionOverride(id).scope).toEqual({ kind: 'app', appId: 'last-used-app' });
    expect(resolveScope(id)).toEqual({ kind: 'app', appId: 'last-used-app' });
    // Changing the seed afterwards must NOT retroactively move this draft.
    selectedScope.value = { kind: 'lucidos' };
    expect(resolveScope(id)).toEqual({ kind: 'app', appId: 'last-used-app' });
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
    inputMode.value = { type: 'coding_agent' };
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

    await sendCompose('t-1', { useCodingAgent: true });

    expect(inputMode.value).toEqual({ type: 'coding_agent' });
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

    expect(inputMode.value).toEqual({ type: 'coding_agent' });
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

    await sendCompose('t-1', { useCodingAgent: false });

    expect(inputMode.value).toEqual({ type: 'do' });
  });
});

/** A CC reasoning-effort (or model) pick made in the compose view is a
 *  PER-DRAFT intent: it lives on THIS draft's override (`composeSelections`),
 *  applies to exactly the thread its send spawns, and is cleared afterward. It
 *  never touches the global `codingAgentPending*` signal (the active-thread
 *  mechanism), so it can't ride onto another draft or the next new thread —
 *  the leak this whole feature exists to prevent. */
describe('CC compose pick is per-draft (no cross-thread leak)', () => {
  let mockFetch: ReturnType<typeof vi.fn>;
  let chatBodies: Array<{ use_coding_agent?: boolean; reasoning_effort?: string; cc_model?: string }>;

  beforeEach(() => {
    chatBodies = [];
    mockFetch = vi.fn((url: string, init?: RequestInit) => {
      const method = (init?.method ?? 'GET').toUpperCase();
      if (typeof url === 'string' && url.endsWith('/chat/stream') && method === 'POST') {
        try { chatBodies.push(JSON.parse(init!.body as string)); } catch { /* ignore */ }
        return Promise.resolve(new Response(JSON.stringify({ event_id: 'evt' }), {
          status: 200, headers: { 'Content-Type': 'application/json' },
        }));
      }
      return Promise.resolve(new Response(null, { status: 200 }));
    });
    globalThis.fetch = mockFetch as unknown as typeof fetch;
    connectionStatus.value = 'connected';
  });

  afterEach(() => {
    globalThis.fetch = originalFetch;
    connectionStatus.value = 'disconnected';
    focusedThreadId.value = null;
    threadMap.value = new Map();
    _resetComposeDraftsForTesting();
    _resetComposeSelectionsForTesting();
    vi.restoreAllMocks();
  });

  function composingCcThread(id: string, text: string): ThreadState {
    return makeThread({
      id, state: 'composing', channel: 'claude_code',
      composeText: text, composeMode: 'claude_code',
    });
  }

  it('carries the draft effort into the spawn and clears it afterward', async () => {
    threadMap.value = new Map<string, ThreadState>().set('cc-1', composingCcThread('cc-1', 'do a thing'));
    focusedThreadId.value = 'cc-1';
    patchComposeSelection('cc-1', { ccReasoningEffort: 'max' });

    await sendCompose('cc-1', { useCodingAgent: true });

    expect(chatBodies).toHaveLength(1);
    expect(chatBodies[0].reasoning_effort).toBe('max'); // the pick reached the spawn
    expect(getComposeSelectionOverride('cc-1').ccReasoningEffort).toBeUndefined(); // ...and was consumed
  });

  it('does NOT leak the pick onto the next new draft', async () => {
    threadMap.value = new Map<string, ThreadState>().set('cc-A', composingCcThread('cc-A', 'first'));
    focusedThreadId.value = 'cc-A';
    patchComposeSelection('cc-A', { ccReasoningEffort: 'max' });
    await sendCompose('cc-A', { useCodingAgent: true });

    // Brand-new compose; the user did NOT pick an effort this time.
    threadMap.value = new Map(threadMap.value).set('cc-B', composingCcThread('cc-B', 'second'));
    focusedThreadId.value = 'cc-B';
    await sendCompose('cc-B', { useCodingAgent: true });

    const ccBodies = chatBodies.filter((b) => b.use_coding_agent);
    expect(ccBodies).toHaveLength(2);
    expect(ccBodies[0].reasoning_effort).toBe('max');     // first draft honored its pick
    expect(ccBodies[1].reasoning_effort).toBeUndefined();  // second falls through to the default
  });

  it('carries a draft model pick into the spawn and clears it (symmetry with effort)', async () => {
    threadMap.value = new Map<string, ThreadState>().set('cc-m', composingCcThread('cc-m', 'pick model once'));
    focusedThreadId.value = 'cc-m';
    patchComposeSelection('cc-m', { ccModel: 'opus[1m]' });

    await sendCompose('cc-m', { useCodingAgent: true });

    expect(chatBodies[0].cc_model).toBe('opus[1m]');
    expect(getComposeSelectionOverride('cc-m').ccModel).toBeUndefined();
  });
});

/** `pendingComposePuts` means "the server has not seen this device's latest
 *  compose intent yet" — every consumer (the `ThreadComposeChanged` SSE guard,
 *  `upsertThread`'s staleness gate, the supersede check) reads it that way and
 *  yields while it is set. It must therefore stay set until the LAST scheduled
 *  write settles, not the first: an edit made while a PUT is in flight schedules
 *  a fresh debounce, and a PUT slower than the debounce can overlap the next one
 *  outright. Releasing on the earlier one's completion advertised "settled" with
 *  a newer write still pending. */
describe('pendingComposePuts covers the LAST pending write, not the first', () => {
  let resolveFirstPut: (() => void) | null = null;

  beforeEach(() => {
    _resetComposeDraftsForTesting();
    composeEditedAt.delete('t-1');
    pendingComposePuts.clear();
    const map = new Map<string, ThreadState>();
    map.set('t-1', makeThread({ id: 't-1', state: 'active' }));
    threadMap.value = map;
    connectionStatus.value = 'connected';
    focusedThreadId.value = 't-1';
    vi.useFakeTimers();
    globalThis.fetch = vi.fn(() => new Promise<Response>((resolve) => {
      resolveFirstPut = () => resolve(new Response(null, { status: 204 }));
    })) as unknown as typeof fetch;
  });

  afterEach(() => {
    vi.useRealTimers();
    globalThis.fetch = originalFetch;
    resolveFirstPut = null;
    pendingComposePuts.clear();
    composeEditedAt.delete('t-1');
    _resetComposeDraftsForTesting();
    threadMap.value = new Map();
    focusedThreadId.value = null;
    connectionStatus.value = 'disconnected';
  });

  it('stays set when a keystroke re-arms the debounce while the first PUT is in flight', async () => {
    updateCompose('t-1', { text: 'first' });
    await vi.advanceTimersByTimeAsync(300);       // debounce fires → PUT in flight
    expect(pendingComposePuts.has('t-1')).toBe(true);

    updateCompose('t-1', { text: 'first and more' });  // re-arms the debounce
    resolveFirstPut?.();
    await vi.advanceTimersByTimeAsync(0);         // the FIRST PUT settles

    expect(pendingComposePuts.has('t-1')).toBe(true);
  });
});
