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

import { applySuggestion, clearSupersededDraft, composeEditedAt, discardCompose, ensureFocusedComposeThread, flushUndeliveredComposeDrafts, pendingComposePuts, prefillCompose, sendCompose, sendFollowup, startSetupInterview, updateCompose, applyRemoteCompose, _composeEpochForTesting, _resetUndeliveredComposeDraftsForTesting, _undeliveredComposeDraftsForTesting } from './compose';
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
      liveEventWaitCount: 0,
      liveEventWaits: [],
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

describe('prefillCompose: seeded-prompt drop-in', () => {
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

describe('applySuggestion: seeding the compose input on the user\'s behalf', () => {
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
      liveEventWaitCount: 0,
      liveEventWaits: [],
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
      liveEventWaitCount: 0,
      liveEventWaits: [],
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
      liveEventWaitCount: 0,
      liveEventWaits: [],
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

/**
 * Compose drafts: type locally, deliver durably.
 *
 * A draft lives ONLY in the `composeDrafts` signal, so the server is its
 * storage and the debounced PUT is the only thing that gets it there. On an
 * installed iOS PWA that PUT fails constantly for reasons that say nothing
 * about the request (WebKit aborts in-flight fetches when it suspends the page;
 * the tunnel to the engine drops on any radio change) and the page is then
 * evicted, taking the only copy of the text with it. The reported symptom was
 * three stacked "Compose sync failed: Load failed" cards during one four-minute
 * outage, with nothing re-sending the draft afterwards.
 *
 * Mirrors the pending-preference-write suite in `preferences.test.ts`.
 */
describe('undelivered compose drafts are parked and re-sent', () => {
  /** Rejects the way a suspended page / dropped tunnel does: no answer, so
   *  `isTransientFetchError` is true and a re-send is owed. A `TimeoutError`
   *  (not a transport `TypeError`) so `mutatingFetchIdempotent`'s own single
   *  retry stays out of the way and each push is exactly one attempt. */
  function noAnswer(): DOMException {
    return new DOMException('Request timed out after 10000ms', 'TimeoutError');
  }

  const composePuts = (m: ReturnType<typeof vi.fn>) =>
    m.mock.calls.filter(([url, init]) =>
      String(url).endsWith('/compose') && (init as RequestInit | undefined)?.method === 'PUT');

  let mockFetch: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    vi.useFakeTimers();
    connectionStatus.value = 'connected';
    focusedThreadId.value = 't-1';
    const map = new Map<string, ThreadState>();
    map.set('t-1', makeActiveThread());
    threadMap.value = map;
    _resetComposeDraftsForTesting();
    _resetUndeliveredComposeDraftsForTesting();
    toasts.value = [];
    mockFetch = vi.fn().mockResolvedValue(new Response(null, { status: 204 }));
    globalThis.fetch = mockFetch as unknown as typeof fetch;
  });

  afterEach(() => {
    vi.useRealTimers();
    globalThis.fetch = originalFetch;
    connectionStatus.value = 'disconnected';
    focusedThreadId.value = null;
    threadMap.value = new Map();
    _resetComposeDraftsForTesting();
    _resetUndeliveredComposeDraftsForTesting();
    toasts.value = [];
    vi.restoreAllMocks();
  });

  it('stays silent on the first failure and parks the thread', async () => {
    mockFetch.mockRejectedValue(noAnswer());

    updateCompose('t-1', { text: 'typed while the tunnel was down' });
    await vi.runAllTimersAsync();

    expect(toasts.value.filter((t) => t.type === 'error')).toEqual([]);
    expect(_undeliveredComposeDraftsForTesting()).toEqual(['t-1']);
  });

  it('escalates ONCE after three failures, however many more follow', async () => {
    mockFetch.mockRejectedValue(noAnswer());

    for (const text of ['a', 'ab', 'abc', 'abcd', 'abcde']) {
      updateCompose('t-1', { text });
      await vi.runAllTimersAsync();
    }

    const errors = toasts.value.filter((t) => t.type === 'error');
    expect(errors).toHaveLength(1);
    expect(errors[0].message).toMatch(/not reaching the engine/i);
    // The reported symptom was a stack of identical cards: the key is what
    // collapses them, and an error toast has no auto-dismiss to hide the bug.
    expect(errors[0].key).toBeTruthy();
  });

  it('surfaces a REFUSAL immediately, without parking it', async () => {
    // The engine answered. No retry can change that and the user is owed the
    // reason, so this keeps today's behaviour (a 410 on a discarded thread, a
    // 409 on an archived one) rather than being swallowed into the queue.
    mockFetch.mockResolvedValue(new Response(JSON.stringify({ error: 'thread discarded' }), { status: 410 }));

    updateCompose('t-1', { text: 'against a dead thread' });
    await vi.runAllTimersAsync();

    const errors = toasts.value.filter((t) => t.type === 'error');
    expect(errors).toHaveLength(1);
    expect(errors[0].message).toMatch(/Compose sync failed: 410 thread discarded/);
    expect(_undeliveredComposeDraftsForTesting()).toEqual([]);
  });

  it('collapses repeated refusals into one card', async () => {
    mockFetch.mockResolvedValue(new Response(JSON.stringify({ error: 'thread discarded' }), { status: 410 }));

    updateCompose('t-1', { text: 'one' });
    await vi.runAllTimersAsync();
    updateCompose('t-1', { text: 'two' });
    await vi.runAllTimersAsync();

    expect(toasts.value.filter((t) => t.type === 'error')).toHaveLength(1);
  });

  it('re-sends the CURRENT draft on flush, then drains and retracts the card', async () => {
    mockFetch.mockRejectedValue(noAnswer());
    for (const text of ['a', 'ab', 'abc']) {
      updateCompose('t-1', { text });
      await vi.runAllTimersAsync();
    }
    expect(toasts.value.filter((t) => t.type === 'error')).toHaveLength(1);
    const before = composePuts(mockFetch).length;

    // The link is back.
    mockFetch.mockResolvedValue(new Response(null, { status: 204 }));
    flushUndeliveredComposeDrafts();
    await vi.runAllTimersAsync();

    const sent = composePuts(mockFetch);
    expect(sent.length).toBe(before + 1);
    // The park holds a thread id, not a snapshot, so the re-send carries the
    // draft as it stands now rather than whichever keystroke happened to fail.
    expect(JSON.parse(String((sent[sent.length - 1][1] as RequestInit).body)).text).toBe('abc');
    expect(_undeliveredComposeDraftsForTesting()).toEqual([]);
    expect(toasts.value.filter((t) => t.type === 'error')).toEqual([]);
  });

  it('re-parks a flush that fails again, and issues nothing when nothing is owed', async () => {
    mockFetch.mockRejectedValue(noAnswer());
    updateCompose('t-1', { text: 'still offline' });
    await vi.runAllTimersAsync();

    flushUndeliveredComposeDrafts();
    await vi.runAllTimersAsync();
    expect(_undeliveredComposeDraftsForTesting()).toEqual(['t-1']);

    // Drained: a later flush must be a no-op, not a stray PUT on every resume.
    _resetUndeliveredComposeDraftsForTesting();
    const before = composePuts(mockFetch).length;
    flushUndeliveredComposeDrafts();
    await vi.runAllTimersAsync();
    expect(composePuts(mockFetch).length).toBe(before);
  });

  it('does not stamp the edit watermark on a re-push', async () => {
    // A re-push is a delivery attempt, not an edit. Re-stamping would move the
    // reference point `draftIsSuperseded` compares against, which is what makes
    // a draft another device already submitted droppable.
    mockFetch.mockRejectedValue(noAnswer());
    updateCompose('t-1', { text: 'owed' });
    await vi.runAllTimersAsync();
    const editedAt = composeEditedAt.get('t-1');

    vi.advanceTimersByTime(60_000);
    flushUndeliveredComposeDrafts();
    await vi.runAllTimersAsync();

    expect(composeEditedAt.get('t-1')).toBe(editedAt);
  });

  it('discarding a thread stops it being owed, so no flush resurrects it', async () => {
    mockFetch.mockRejectedValue(noAnswer());
    updateCompose('t-1', { text: 'never mind' });
    await vi.runAllTimersAsync();
    expect(_undeliveredComposeDraftsForTesting()).toEqual(['t-1']);

    mockFetch.mockResolvedValue(new Response(null, { status: 204 }));
    await discardCompose('t-1');
    expect(_undeliveredComposeDraftsForTesting()).toEqual([]);

    const before = composePuts(mockFetch).length;
    flushUndeliveredComposeDrafts();
    await vi.runAllTimersAsync();
    expect(composePuts(mockFetch).length).toBe(before);
  });

  it('sending a thread stops it being owed, so no flush re-posts the draft', async () => {
    mockFetch.mockRejectedValue(noAnswer());
    updateCompose('t-1', { text: 'about to send' });
    await vi.runAllTimersAsync();
    expect(_undeliveredComposeDraftsForTesting()).toEqual(['t-1']);

    mockFetch.mockResolvedValue(new Response(JSON.stringify({ event_id: 'e-1' }), { status: 200 }));
    await sendCompose('t-1', {});

    expect(_undeliveredComposeDraftsForTesting()).toEqual([]);
    // The send owes exactly one further write, the cleared draft, and it must
    // be the last word on this thread's compose state. What must NOT happen is
    // the flush re-posting the text that was just sent.
    const before = composePuts(mockFetch).length;
    flushUndeliveredComposeDrafts();
    await vi.runAllTimersAsync();
    const after = composePuts(mockFetch);
    expect(after.length).toBe(before + 1);
    expect(JSON.parse(String((after[after.length - 1][1] as RequestInit).body)).text).toBe('');
  });

  it('a draft the thread history proves was submitted stops being owed', async () => {
    // The peer-submission case. This device typed text, got it acked, edited it,
    // then went offline while another device sent the very same text. Without
    // the drop, the reconnect flush would push the sent message back up as a
    // live draft and it would reappear on every device.
    updateCompose('t-1', { text: 'shared text' });
    await vi.runAllTimersAsync();                  // acked: serverDraft = 'shared text'

    mockFetch.mockRejectedValue(noAnswer());
    updateCompose('t-1', { text: 'shared text v2' });
    await vi.runAllTimersAsync();                  // fails: parked
    expect(_undeliveredComposeDraftsForTesting()).toEqual(['t-1']);

    // The peer's submission lands: compose cleared server-side, and the thread's
    // own history now carries exactly this draft's content.
    applyRemoteCompose('t-1', { text: '', image_hashes: [], mode: null });
    const thread = threadMap.value.get('t-1')!;
    thread.events.set(9 as never, {
      type: 'MessageReceived',
      text: 'shared text v2',
      user_image_hashes: [],
      created: '2099-01-01T00:00:00Z',
    } as never);

    clearSupersededDraft('t-1');

    expect(_undeliveredComposeDraftsForTesting()).toEqual([]);
    const before = composePuts(mockFetch).length;
    flushUndeliveredComposeDrafts();
    await vi.runAllTimersAsync();
    expect(composePuts(mockFetch).length).toBe(before);
  });
});

/** The park must not outlive the thread it belongs to. `pushNow` early-returns
 *  for a thread that is gone (its POST /threads failed and the optimistic entry
 *  rolled back) or already discarded, so it never settles and the entry would
 *  sit in the queue forever, holding the unreachable card up behind it. */
describe('flushUndeliveredComposeDrafts drops what is no longer owed', () => {
  let mockFetch: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    vi.useFakeTimers();
    connectionStatus.value = 'connected';
    focusedThreadId.value = 't-1';
    const map = new Map<string, ThreadState>();
    map.set('t-1', makeActiveThread());
    threadMap.value = map;
    _resetComposeDraftsForTesting();
    _resetUndeliveredComposeDraftsForTesting();
    toasts.value = [];
    mockFetch = vi.fn().mockRejectedValue(new DOMException('Request timed out after 10000ms', 'TimeoutError'));
    globalThis.fetch = mockFetch as unknown as typeof fetch;
  });

  afterEach(() => {
    vi.useRealTimers();
    globalThis.fetch = originalFetch;
    connectionStatus.value = 'disconnected';
    focusedThreadId.value = null;
    threadMap.value = new Map();
    _resetComposeDraftsForTesting();
    _resetUndeliveredComposeDraftsForTesting();
    toasts.value = [];
    vi.restoreAllMocks();
  });

  it('drops a parked thread that has vanished from threadMap, and retracts the card', async () => {
    for (const text of ['a', 'ab', 'abc']) {
      updateCompose('t-1', { text });
      await vi.runAllTimersAsync();
    }
    expect(_undeliveredComposeDraftsForTesting()).toEqual(['t-1']);
    expect(toasts.value.filter((t) => t.type === 'error')).toHaveLength(1);

    threadMap.value = new Map();
    flushUndeliveredComposeDrafts();
    await vi.runAllTimersAsync();

    expect(_undeliveredComposeDraftsForTesting()).toEqual([]);
    expect(toasts.value.filter((t) => t.type === 'error')).toEqual([]);
  });
});

/**
 * Compose writes for one thread are SERIALIZED.
 *
 * A draft lives only in the `composeDrafts` signal, so the debounced PUT is the
 * only thing that persists it, and the engine keeps whichever write it applies
 * LAST. Overlapping writes can be applied in either order: on a stalled link a
 * PUT far slower than the 250ms debounce is still running when the next one
 * goes out, and the older text can win.
 *
 * On 2026-08-06 that shipped as a message that was both sent and still sitting
 * in the composer, holding an OLDER revision of the sent text: the pre-send
 * draft PUT landed after the message cleared compose, and the reconnect resync
 * staged the resurrected draft back into the box.
 *
 * So at most one write is in flight per thread, and a newer intent raised while
 * one runs is issued afterwards, reading the draft as it stands then. This
 * replaces the older `latestComposePushSeq` guard, which made an out-of-order
 * ANSWER harmless without stopping the out-of-order WRITE. Serializing removes
 * the overlap instead of tolerating it, so that guard is gone.
 */
describe('compose writes for a thread are serialized', () => {
  let resolvers: Array<{ resolve: (r: Response) => void; reject: (e: unknown) => void; body: string }>;
  let mockFetch: ReturnType<typeof vi.fn>;

  const composePuts = () => resolvers.map((r) => JSON.parse(r.body).text as string);

  beforeEach(() => {
    vi.useFakeTimers();
    connectionStatus.value = 'connected';
    focusedThreadId.value = 't-1';
    const map = new Map<string, ThreadState>();
    map.set('t-1', makeActiveThread());
    threadMap.value = map;
    _resetComposeDraftsForTesting();
    _resetUndeliveredComposeDraftsForTesting();
    toasts.value = [];
    resolvers = [];
    // Every compose PUT hangs until the test resolves it by hand, which is how
    // a stalled link is modelled here.
    mockFetch = vi.fn((_url: string, init?: RequestInit) => new Promise<Response>((resolve, reject) => {
      resolvers.push({ resolve, reject, body: String(init?.body ?? '') });
    }));
    globalThis.fetch = mockFetch as unknown as typeof fetch;
  });

  afterEach(() => {
    vi.useRealTimers();
    globalThis.fetch = originalFetch;
    connectionStatus.value = 'disconnected';
    focusedThreadId.value = null;
    threadMap.value = new Map();
    _resetComposeDraftsForTesting();
    _resetUndeliveredComposeDraftsForTesting();
    toasts.value = [];
    vi.restoreAllMocks();
  });

  it('starts no second write while one is still in flight', async () => {
    updateCompose('t-1', { text: 'abc' });
    await vi.advanceTimersByTimeAsync(300);        // the write for 'abc' goes out
    expect(resolvers).toHaveLength(1);

    updateCompose('t-1', { text: 'abcd' });
    await vi.advanceTimersByTimeAsync(300);        // its debounce elapses...

    expect(resolvers).toHaveLength(1);             // ...but nothing overlaps it
    expect(pendingComposePuts.has('t-1')).toBe(true);
  });

  it('the queued write carries the NEWEST draft, not the intent that queued it', async () => {
    updateCompose('t-1', { text: 'abc' });
    await vi.advanceTimersByTimeAsync(300);
    updateCompose('t-1', { text: 'abcd' });
    await vi.advanceTimersByTimeAsync(300);
    updateCompose('t-1', { text: 'abcde' });
    await vi.advanceTimersByTimeAsync(300);

    resolvers[0].resolve(new Response(null, { status: 204 }));
    await vi.runAllTimersAsync();

    // Two writes total: the one that was in flight, then ONE catching up on
    // everything typed while it ran. Both intents coalesced, and the write that
    // went out read the draft at issue time.
    expect(composePuts()).toEqual(['abc', 'abcde']);
  });

  it('the engine ends up holding the newest text, never an older revision', async () => {
    updateCompose('t-1', { text: 'You can have both' });
    await vi.advanceTimersByTimeAsync(300);
    updateCompose('t-1', { text: 'You can have both from us' });
    await vi.advanceTimersByTimeAsync(300);

    // The first write finally lands, long after the second was typed.
    resolvers[0].resolve(new Response(null, { status: 204 }));
    await vi.runAllTimersAsync();
    resolvers[1]?.resolve(new Response(null, { status: 204 }));
    await vi.runAllTimersAsync();

    const applied = composePuts();
    expect(applied[applied.length - 1]).toBe('You can have both from us');
  });

  it('keeps pendingComposePuts set across the whole queued window', async () => {
    updateCompose('t-1', { text: 'abc' });
    await vi.advanceTimersByTimeAsync(300);
    updateCompose('t-1', { text: 'abcd' });
    await vi.advanceTimersByTimeAsync(300);       // queued behind the in-flight one

    resolvers[0].resolve(new Response(null, { status: 204 }));
    await vi.advanceTimersByTimeAsync(0);         // the FIRST write settles

    // A queued write is still owed, so every inbound clobber guard must keep
    // yielding: the engine has NOT seen our latest intent yet.
    expect(pendingComposePuts.has('t-1')).toBe(true);

    resolvers[1].resolve(new Response(null, { status: 204 }));
    await vi.runAllTimersAsync();
    expect(pendingComposePuts.has('t-1')).toBe(false);
  });

  it('the tab-close flush carries a write queued behind an in-flight one', async () => {
    // Serialization added a second place an unsent intent can live: a debounce
    // that fired while a write was running leaves no timer, only a queued
    // intent. A flush that looked at timers alone would drop exactly the text
    // this change is about, since a draft lives nowhere but this page.
    updateCompose('t-1', { text: 'hello' });
    await vi.advanceTimersByTimeAsync(300);        // 'hello' dispatched, hanging
    updateCompose('t-1', { text: 'hello world' });
    await vi.advanceTimersByTimeAsync(300);        // queued, no timer left

    window.dispatchEvent(new Event('pagehide'));

    const flushed = mockFetch.mock.calls
      .filter(([, init]) => (init as RequestInit | undefined)?.keepalive)
      .map(([, init]) => JSON.parse(String((init as RequestInit).body)).text as string);
    expect(flushed).toEqual(['hello world']);
  });

  it('a failed write still parks the thread, and the queued one still goes out', async () => {
    updateCompose('t-1', { text: 'abc' });
    await vi.advanceTimersByTimeAsync(300);
    updateCompose('t-1', { text: 'abcd' });
    await vi.advanceTimersByTimeAsync(300);

    resolvers[0].reject(new DOMException('Request timed out after 10000ms', 'TimeoutError'));
    await vi.advanceTimersByTimeAsync(0);
    expect(_undeliveredComposeDraftsForTesting()).toEqual(['t-1']);

    resolvers[1].resolve(new Response(null, { status: 204 }));
    await vi.runAllTimersAsync();

    // The queued write carried the text the user can see, and it landed, so
    // nothing is owed any more.
    expect(composePuts()).toEqual(['abc', 'abcd']);
    expect(_undeliveredComposeDraftsForTesting()).toEqual([]);
    expect(toasts.value.filter((t) => t.type === 'error')).toEqual([]);
  });
});

/**
 * The write fence, from the client's side.
 *
 * A `412` means a submission consumed the thread's compose slot after this
 * write was composed, so the engine dropped it and handed back the current
 * *compose epoch*. That is not a refusal the user can act on: the text is still
 * theirs and still unsent, so the client adopts the epoch and re-issues in
 * silence. Treating it as an ordinary rejection would toast a stack of
 * "Compose sync failed" cards and abandon the draft.
 */
describe('a stale-epoch refusal resyncs instead of failing', () => {
  let mockFetch: ReturnType<typeof vi.fn>;

  const composeBodies = () => mockFetch.mock.calls
    .filter(([url, init]) => String(url).endsWith('/compose') && (init as RequestInit | undefined)?.method === 'PUT')
    .map(([, init]) => JSON.parse(String((init as RequestInit).body)));

  beforeEach(() => {
    vi.useFakeTimers();
    connectionStatus.value = 'connected';
    focusedThreadId.value = 't-1';
    const map = new Map<string, ThreadState>();
    map.set('t-1', makeActiveThread());
    threadMap.value = map;
    _resetComposeDraftsForTesting();
    _resetUndeliveredComposeDraftsForTesting();
    toasts.value = [];
  });

  afterEach(() => {
    vi.useRealTimers();
    globalThis.fetch = originalFetch;
    connectionStatus.value = 'disconnected';
    focusedThreadId.value = null;
    threadMap.value = new Map();
    _resetComposeDraftsForTesting();
    _resetUndeliveredComposeDraftsForTesting();
    toasts.value = [];
    vi.restoreAllMocks();
  });

  it('adopts the epoch, re-issues the draft, and says nothing to the user', async () => {
    let calls = 0;
    mockFetch = vi.fn(() => {
      calls += 1;
      return Promise.resolve(calls === 1
        ? new Response(JSON.stringify({ error: 'stale', compose_epoch: 7 }), { status: 412 })
        : new Response(null, { status: 204 }));
    });
    globalThis.fetch = mockFetch as unknown as typeof fetch;

    updateCompose('t-1', { text: 'typed while behind a submission' });
    await vi.runAllTimersAsync();

    const bodies = composeBodies();
    expect(bodies).toHaveLength(2);
    expect(bodies[0].compose_epoch).toBeUndefined();      // nothing heard yet
    expect(bodies[1].compose_epoch).toBe(7);              // adopted from the 412
    expect(bodies[1].text).toBe('typed while behind a submission');
    expect(toasts.value.filter((t) => t.type === 'error')).toEqual([]);
    expect(_undeliveredComposeDraftsForTesting()).toEqual([]);
    expect(_composeEpochForTesting('t-1')).toBe(7);
  });

  it('does not re-push a draft the submission that moved the epoch already sent', async () => {
    // The peer-submission case, reached through the retry. This device typed a
    // draft, the same user sent that exact text from another device, and this
    // device's write is refused. Re-issuing would put the sent message back as
    // a live draft on every device, which is the ghost-draft class this whole
    // change exists to close. The supersede rule must therefore actually run
    // here, and it cannot use its ordinary "a write of ours is in flight" bail:
    // a write is in flight for the entire life of the push.
    const sent = 'shared text';
    const thread = makeActiveThread();
    thread.events.set(9 as never, {
      type: 'MessageReceived',
      text: sent,
      user_image_hashes: [],
      created: '2099-01-01T00:00:00Z',
    } as never);
    threadMap.value = new Map([['t-1', thread]]);

    mockFetch = vi.fn(() => Promise.resolve(
      new Response(JSON.stringify({ error: 'stale', compose_epoch: 4 }), { status: 412 }),
    ));
    globalThis.fetch = mockFetch as unknown as typeof fetch;

    updateCompose('t-1', { text: sent });
    await vi.runAllTimersAsync();

    // Exactly one attempt: refused, recognised as already-submitted, dropped.
    expect(composeBodies()).toHaveLength(1);
    expect(getDraft('t-1').text).toBe('');
    expect(_undeliveredComposeDraftsForTesting()).toEqual([]);
    expect(toasts.value.filter((t) => t.type === 'error')).toEqual([]);
  });

  it('parks the draft rather than spinning when the slot keeps being consumed', async () => {
    let epoch = 1;
    mockFetch = vi.fn(() => Promise.resolve(
      new Response(JSON.stringify({ error: 'stale', compose_epoch: epoch++ }), { status: 412 }),
    ));
    globalThis.fetch = mockFetch as unknown as typeof fetch;

    updateCompose('t-1', { text: 'racing a busy peer' });
    await vi.runAllTimersAsync();

    // Bounded: the first attempt plus its two retries, then parked for the next
    // resume flush rather than looping against a peer that keeps submitting.
    expect(composeBodies()).toHaveLength(3);
    expect(_undeliveredComposeDraftsForTesting()).toEqual(['t-1']);
    expect(toasts.value.filter((t) => t.type === 'error')).toEqual([]);
  });
});

/**
 * Every send ends with a compose write carrying the CLEARED draft.
 *
 * Serialization puts that write after every earlier one, so it is the last
 * thing the engine applies for the thread and a pre-send draft cannot be the
 * resting state. Both send paths owe it, which is the whole reason they share
 * one helper: `sendFollowup` had it (via its `updateCompose('')`) and
 * `sendCompose` did not, and `sendCompose` is the first-send path where the
 * user types and sends in one gesture, so its write is the one most likely to
 * still be in flight.
 */
describe('a send leaves the engine holding an empty draft', () => {
  let mockFetch: ReturnType<typeof vi.fn>;

  const composePutTexts = () => mockFetch.mock.calls
    .filter(([url, init]) => String(url).endsWith('/compose') && (init as RequestInit | undefined)?.method === 'PUT')
    .map(([, init]) => JSON.parse(String((init as RequestInit).body)).text as string);

  beforeEach(() => {
    vi.useFakeTimers();
    connectionStatus.value = 'connected';
    focusedThreadId.value = 't-1';
    _resetComposeDraftsForTesting();
    _resetUndeliveredComposeDraftsForTesting();
    _resetComposeSelectionsForTesting();
    toasts.value = [];
    mockFetch = vi.fn().mockResolvedValue(new Response(JSON.stringify({ event_id: 'e-1' }), { status: 200 }));
    globalThis.fetch = mockFetch as unknown as typeof fetch;
  });

  afterEach(() => {
    vi.useRealTimers();
    globalThis.fetch = originalFetch;
    connectionStatus.value = 'disconnected';
    focusedThreadId.value = null;
    threadMap.value = new Map();
    _resetComposeDraftsForTesting();
    _resetUndeliveredComposeDraftsForTesting();
    _resetComposeSelectionsForTesting();
    toasts.value = [];
    vi.restoreAllMocks();
  });

  it('sendCompose writes the cleared draft after the send', async () => {
    threadMap.value = new Map([['t-1', makeThread({ id: 't-1', state: 'composing', composeText: 'the draft' })]]);

    await sendCompose('t-1', {});
    await vi.runAllTimersAsync();

    const texts = composePutTexts();
    expect(texts.length).toBeGreaterThan(0);
    expect(texts[texts.length - 1]).toBe('');
  });

  it('sendFollowup writes the cleared draft after the send', async () => {
    threadMap.value = new Map([['t-1', makeActiveThread({ id: 't-1', composeText: 'a follow-up' })]]);

    await sendFollowup('t-1', 'a follow-up');
    await vi.runAllTimersAsync();

    const texts = composePutTexts();
    expect(texts.length).toBeGreaterThan(0);
    expect(texts[texts.length - 1]).toBe('');
  });

  it('does not carry the draft dropdown picks back onto the sent thread', async () => {
    // The `MessageReceived` projection sets `compose_selection = NULL` on send.
    // A trailing clear that still carried the draft's picks would COALESCE them
    // straight back onto the row, so the write is scheduled only after
    // `clearComposeSelection` has consumed them.
    threadMap.value = new Map([['t-1', makeThread({ id: 't-1', state: 'composing', composeText: 'the draft' })]]);
    patchComposeSelection('t-1', { model: 'claude-opus-5' });

    await sendCompose('t-1', {});
    await vi.runAllTimersAsync();

    const bodies = mockFetch.mock.calls
      .filter(([url, init]) => String(url).endsWith('/compose') && (init as RequestInit | undefined)?.method === 'PUT')
      .map(([, init]) => JSON.parse(String((init as RequestInit).body)));
    expect(bodies.length).toBeGreaterThan(0);
    expect(bodies[bodies.length - 1].selection).toBeUndefined();
    expect(bodies[bodies.length - 1].text).toBe('');
  });

  it('carries a follow-up typed straight after the send, rather than an empty draft', async () => {
    threadMap.value = new Map([['t-1', makeActiveThread({ id: 't-1', composeText: 'a follow-up' })]]);

    await sendFollowup('t-1', 'a follow-up');
    updateCompose('t-1', { text: 'and one more thing' });   // inside the debounce
    await vi.runAllTimersAsync();

    // The write re-reads the draft when it fires, so the two intents coalesce
    // into one write carrying what the user can actually see.
    const texts = composePutTexts();
    expect(texts[texts.length - 1]).toBe('and one more thing');
  });

});

/**
 * `sendCompose` must not race `POST /threads`.
 *
 * `ensureFocusedComposeThread` fires the thread creation WITHOUT awaiting it and
 * parks the promise in `pendingThreadStarts`; the draft PUT awaits that promise
 * inside `pushNow`. Typing hid the gap for every pre-existing caller, because a
 * human needs far longer to reach Send than a POST needs to settle. The setup
 * interview composes and sends in one gesture and `sendCompose` cancels the
 * pending PUT on its way through, so nothing was left waiting on the row and the
 * chat POST could reach the backend before the thread existed.
 */
describe('sendCompose waits for the thread row before the chat POST', () => {
  let mockFetch: ReturnType<typeof vi.fn>;
  let releaseThreadStart: ((value: Response) => void) | null;

  const chatCalls = () => mockFetch.mock.calls.filter(([url]) =>
    typeof url === 'string' && url.endsWith('/chat/stream'));

  beforeEach(() => {
    releaseThreadStart = null;
    mockFetch = vi.fn().mockImplementation((url: unknown, init?: RequestInit) => {
      // Hold POST /threads open so the race window is wide instead of timing-dependent.
      if (typeof url === 'string' && url.endsWith('/threads') && init?.method === 'POST') {
        return new Promise<Response>((resolve) => { releaseThreadStart = resolve; });
      }
      return Promise.resolve(new Response(null, { status: 200 }));
    });
    globalThis.fetch = mockFetch as unknown as typeof fetch;
    connectionStatus.value = 'connected';
    focusedThreadId.value = null;
    threadMap.value = new Map();
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

  it('holds the chat POST until POST /threads settles, then sends it', async () => {
    const started = startSetupInterview();
    // Let every already-resolved microtask drain. The chat POST must still be
    // unsent: the thread row does not exist server-side yet.
    await Promise.resolve();
    await Promise.resolve();
    expect(chatCalls(), 'chat POST fired before the thread row existed').toHaveLength(0);

    releaseThreadStart!(new Response(null, { status: 200 }));
    await expect(started).resolves.toBe(true);
    expect(chatCalls(), 'chat POST never fired after the thread row landed').toHaveLength(1);
  });

  it('reports a failed thread start instead of silently sending nothing', async () => {
    const started = startSetupInterview();
    await Promise.resolve();
    releaseThreadStart!(new Response(null, { status: 500 }));

    await expect(started).resolves.toBe(false);
    expect(chatCalls(), 'chat POST fired despite the thread row failing').toHaveLength(0);
    expect(toasts.value.filter((t) => t.type === 'error').length).toBeGreaterThan(0);
  });
});

/**
 * A starter suggestion composes a NEW message, so it belongs in a draft, never
 * appended to a thread that has already been sent.
 *
 * `ensureFocusedComposeThread` returns the focused id whatever its state, which
 * is right for typing (an active thread's composer writes a follow-up draft onto
 * that thread) and wrong here. The setup interview's header button is a
 * permanent control, so it can be tapped with any thread focused, and it aimed
 * the interview at whatever the user was looking at: on a coding-agent thread
 * the engine's continuity lock rejected the send ("Failed to send message: 409",
 * the 2026-08-05 iOS PWA report) and the rollback left the thread rendering as a
 * Lucidos Agent thread; on a chat thread the interview landed silently in an
 * unrelated conversation. `navigation-request`'s `new-chat` branch already drops
 * focus for exactly this reason.
 */
describe('a suggestion never lands on an already-sent thread', () => {
  let mockFetch: ReturnType<typeof vi.fn>;

  const chatBodies = () => mockFetch.mock.calls
    .filter(([url]) => typeof url === 'string' && url.endsWith('/chat/stream'))
    .map(([, init]) => JSON.parse((init as RequestInit).body as string));

  beforeEach(() => {
    mockFetch = vi.fn().mockResolvedValue(new Response(null, { status: 200 }));
    globalThis.fetch = mockFetch as unknown as typeof fetch;
    connectionStatus.value = 'connected';
    threadMap.value = new Map([['cc-1', makeActiveThread({
      id: 'cc-1',
      channel: 'claude_code',
      codingAgent: 'claude-code',
    })]]);
    focusedThreadId.value = 'cc-1';
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

  it('startSetupInterview sends on a fresh thread, not the focused coding-agent one', async () => {
    await expect(startSetupInterview()).resolves.toBe(true);

    const bodies = chatBodies();
    expect(bodies).toHaveLength(1);
    expect(bodies[0].thread_id, 'interview sent as a follow-up on the open thread')
      .not.toBe('cc-1');
    expect(bodies[0].use_coding_agent).toBeUndefined();
  });

  it('leaves the focused coding-agent thread exactly as it found it', async () => {
    await startSetupInterview();

    const cc = threadMap.value.get('cc-1')!;
    expect(cc.meta.state, 'open thread demoted to a draft by the interview').toBe('active');
    expect(cc.meta.channel, 'coding-agent thread relabelled as a Lucidos Agent thread')
      .toBe('claude_code');
    expect(getDraft('cc-1').text, 'interview prompt written into the open thread').toBe('');
  });

  it('applySuggestion prefills a fresh draft rather than the open thread', async () => {
    await expect(applySuggestion('summarize my week')).resolves.toBe(true);

    const draftId = focusedThreadId.value!;
    expect(draftId).not.toBe('cc-1');
    expect(getDraft(draftId).text).toBe('summarize my week');
    expect(getDraft('cc-1').text).toBe('');
  });

  it('does not stop to ask: an open thread is not a draft to replace', async () => {
    await applySuggestion('summarize my week');
    expect(confirmState.value.visible, 'confirmed a replace of a thread with no draft').toBe(false);
  });

  it('leaves a half-typed follow-up on the open thread alone', async () => {
    setDraft('cc-1', { text: 'and also rename the button', image_hashes: ['h1'], mode: null });

    await applySuggestion('summarize my week');

    // The confirm exists to protect a DRAFT the suggestion would replace. A
    // follow-up being typed into an open thread is not that: the suggestion is
    // going somewhere else entirely, so there is nothing to ask about and
    // nothing to overwrite.
    expect(confirmState.value.visible).toBe(false);
    expect(getDraft('cc-1').text).toBe('and also rename the button');
    expect(getDraft('cc-1').image_hashes).toEqual(['h1']);
  });
});

/**
 * A compose write carries a `mode` only when the mode CHANGES.
 *
 * `POST /threads` already stores the channel, and the endpoint COALESCEs, so a
 * keystroke re-sending the same value changes nothing server-side. It did have
 * one effect. A write composed before a send and landing after it was read as
 * an attempt to change a sent thread's channel. The user got a "Compose sync
 * failed" card for typing. See
 * `docs/plans/2026-08-26-compose-mode-lock-masks-the-stale-write-fence.md`.
 */
describe('a compose write states the mode only when it changes', () => {
  let mockFetch: ReturnType<typeof vi.fn>;

  const composeModes = () => mockFetch.mock.calls
    .filter(([url, init]) => String(url).endsWith('/compose') && (init as RequestInit | undefined)?.method === 'PUT')
    .map(([, init]) => JSON.parse(String((init as RequestInit).body)).mode as string | null);

  beforeEach(() => {
    vi.useFakeTimers();
    connectionStatus.value = 'connected';
    focusedThreadId.value = null;
    threadMap.value = new Map();
    inputMode.value = { type: 'do' };
    _resetComposeDraftsForTesting();
    _resetComposeSelectionsForTesting();
    _resetUndeliveredComposeDraftsForTesting();
    _resetThreadNavForTesting();
    toasts.value = [];
    mockFetch = vi.fn(() => Promise.resolve(new Response(null, { status: 204 })));
    globalThis.fetch = mockFetch as unknown as typeof fetch;
  });

  afterEach(() => {
    vi.useRealTimers();
    globalThis.fetch = originalFetch;
    connectionStatus.value = 'disconnected';
    focusedThreadId.value = null;
    threadMap.value = new Map();
    _resetComposeDraftsForTesting();
    _resetComposeSelectionsForTesting();
    _resetUndeliveredComposeDraftsForTesting();
    _resetThreadNavForTesting();
    localStorage.removeItem(FOCUSED_THREAD_KEY);
    toasts.value = [];
    vi.restoreAllMocks();
  });

  it('sends no mode on a keystroke: the POST that created the thread stored it', async () => {
    const id = ensureFocusedComposeThread();
    updateCompose(id, { text: 'typing' });
    await vi.runAllTimersAsync();

    expect(composeModes()).toEqual([null]);
  });

  it('sends the mode on a toggle, then stops once the engine has it', async () => {
    const id = ensureFocusedComposeThread();
    updateCompose(id, { mode: 'claude_code' });
    await vi.runAllTimersAsync();
    expect(composeModes()).toEqual(['claude_code']);

    updateCompose(id, { text: 'now typing' });
    await vi.runAllTimersAsync();
    expect(composeModes()).toEqual(['claude_code', null]);
  });

  it('re-states a mode the engine never acknowledged', async () => {
    const id = ensureFocusedComposeThread();
    await vi.runAllTimersAsync();

    // The toggle's write is dropped in transit, so nothing proves the engine
    // stored it. A mode we only ever failed to deliver must keep going out.
    let dropComposeWrites = true;
    mockFetch.mockImplementation((url: string, init?: RequestInit) => {
      const isComposePut = String(url).endsWith('/compose') && (init?.method ?? '') === 'PUT';
      if (isComposePut && dropComposeWrites) return Promise.reject(new TypeError('Load failed'));
      return Promise.resolve(new Response(null, { status: 204 }));
    });

    updateCompose(id, { mode: 'claude_code' });
    await vi.runAllTimersAsync();

    dropComposeWrites = false;
    updateCompose(id, { text: 'still owed' });
    await vi.runAllTimersAsync();

    // Every attempt states the mode, the landing one included. The count is
    // left alone: `mutatingFetchIdempotent` retries a dropped write once, and
    // that is its business rather than this rule's.
    expect(composeModes()).not.toEqual([]);
    expect(composeModes().every((m) => m === 'claude_code')).toBe(true);
  });
});
