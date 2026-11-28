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

import { seedSuggestion, startFreshDraft, discardCompose, ensureFocusedComposeThread, flushUndeliveredComposeDrafts, awaitThreadStarted, isThreadStartPending, prefillCompose, sendCompose, sendFollowup, startSetupInterview, updateCompose, selectComposeCodingAgent, _resetUndeliveredComposeDraftsForTesting, _undeliveredComposeDraftsForTesting } from './compose';
import { _resetRowIntentsForTesting } from '../rowIntents';
import { acceptComposeReport, composeEditSeq, isComposeIntentPending, knownComposeEpoch, _resetComposeSyncForTesting } from '../composeSync';
import { focusThread, unfocusThread } from './threads';
import { connectionStatus, preferences, selectedCodingAgent, confirmState, effectiveThreadStatus, focusedThreadId, focusedPane, inputMode, threadMap, selectedScope, FOCUSED_THREAD_KEY, toasts } from '../store';
import { promptCaretRequest } from '../../components/chat/promptValueSync';
import { applyCodingAgentDefault, patchComposeSelection, getComposeSelectionOverride, resolveCodingAgent, resolveProvider, resolveScope, _resetComposeSelectionsForTesting } from '../composeSelections';
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
import { cancelUnsentHookInput, retryUnsentMessage } from './chat';
import { upsertThread } from './thread-loading';
import { handleThreadEvent } from './thread-sync';
import { composeViewActive } from '../store';
import { unsentMessages } from '../unsentMessages';
import type { ThreadSummary } from '../../api/threads';

const originalFetch = globalThis.fetch;

afterEach(() => _resetRowIntentsForTesting());

let peerRevision = 1000;
function peerReport(threadId: string, text: string): void {
  peerRevision += 1;
  acceptComposeReport(threadId, { revision: peerRevision, epoch: null, text, imageHashes: [], mode: null, selection: null });
}

const chatAccepted = () => new Response(JSON.stringify({ event_id: 'e-1' }), { status: 200 });

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
      summaryVersion: 0,
      codingAgentChangeState: { kind: 'none' },
      codingAgentIsExternalRepo: false,
      lastRevivedAt: '',
      messageCount: 0,
      section: 'archived',
      activeChildrenCount: 0,
      totalChildrenCount: 0,
      blockingDescendantCount: 0, attentionDescendantCount: 0, archivableSubThreadCount: 0,
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

function staleComposingSummary(threadId: string, composeText: string): ThreadSummary {
  return {
    thread_id: threadId,
    title: '',
    channel: 'chat',
    initiator: 'user',
    created_at: '2026-06-15T16:35:54.000Z',
    last_activity: '2026-06-15T16:35:54.000Z',
    message_count: 0,
    section: 'inbox',
    active_children_count: 0,
    total_children_count: 0,
    blocking_descendant_count: 0,
    attention_descendant_count: 0,
    archivable_sub_thread_count: 0,
    live_event_wait_count: 0,
    status: 'idle',
    summary_version: 0,
    coding_agent_change_state: { kind: 'none' },
    coding_agent_is_external_repo: false,
    last_revived_at: null,
    state: 'composing',
    compose_text: composeText,
    compose_images: [],
  };
}

describe('sendFollowup clears the active-thread draft', () => {
  let mockFetch: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    mockFetch = vi.fn(() => Promise.resolve(chatAccepted()));
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

    const draft = getDraft('t-1');
    expect(draft.text).toBe('');
    expect(draft.image_hashes).toEqual([]);

    resolveSend!(chatAccepted());
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
    expect(getDraft('t-1').image_hashes).toEqual([]);
  });
});

describe('seedSuggestion: seeding the compose input on the user\'s behalf', () => {
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

  it('no draft in progress: drops the text in and asks nothing', async () => {
    const seqBefore = promptCaretRequest.value.seq;

    const seeded = seedSuggestion('Build me an app that tracks my reading list.');

    expect(confirmState.value.visible).toBe(false);
    const id = focusedThreadId.value!;
    expect(id).toBeTruthy();
    expect(seeded).toBe(id);
    expect(getDraft(id).text).toBe('Build me an app that tracks my reading list.');
    expect(promptCaretRequest.value.seq).toBe(seqBefore + 1);
    const chatCall = mockFetch.mock.calls.find(([url]) =>
      typeof url === 'string' && url.endsWith('/chat/stream'));
    expect(chatCall, 'seedSuggestion must not POST a chat message').toBeUndefined();
  });

  it('sets the destination to the Lucidos Agent (chat channel)', async () => {
    inputMode.value = { type: 'coding_agent' };
    const map = new Map<string, ThreadState>();
    map.set('cc-1', makeThread({ id: 'cc-1', state: 'composing', channel: 'claude_code', composeMode: 'claude_code' }));
    threadMap.value = map;
    focusedThreadId.value = 'cc-1';

    const seeded = seedSuggestion('Tell me how to set up Lucidos for mobile access.');

    expect(seeded).toBe('cc-1');
    expect(inputMode.value).toEqual({ type: 'do' });
    expect(getDraft('cc-1').mode).toBe('lucidos');
    expect(getDraft('cc-1').text).toBe('Tell me how to set up Lucidos for mobile access.');
  });

  it('draft in progress: seeds a NEW draft and leaves the old one whole', () => {
    const map = new Map<string, ThreadState>();
    map.set('t-1', makeThread({ state: 'composing', composeText: 'my own idea', composeImages: ['iVBORfake'] }));
    threadMap.value = map;
    focusedThreadId.value = 't-1';

    const seeded = seedSuggestion('Where can I download apps?');

    expect(confirmState.value.visible).toBe(false);
    expect(seeded).not.toBe('t-1');
    expect(focusedThreadId.value).toBe(seeded);
    expect(getDraft(seeded).text).toBe('Where can I download apps?');
    expect(getDraft('t-1').text).toBe('my own idea');
    expect(getDraft('t-1').image_hashes).toEqual(['iVBORfake']);
  });

  it('aims the new draft at the Lucidos Agent', () => {
    inputMode.value = { type: 'coding_agent' };
    const map = new Map<string, ThreadState>();
    map.set('cc-1', makeThread({ id: 'cc-1', state: 'composing', channel: 'claude_code', composeMode: 'claude_code', composeText: 'my own idea' }));
    threadMap.value = map;
    focusedThreadId.value = 'cc-1';

    const seeded = seedSuggestion('Where can I download apps?');

    expect(seeded).not.toBe('cc-1');
    expect(getDraft(seeded).mode).toBe('lucidos');
    expect(getDraft('cc-1').text).toBe('my own idea');
  });
});

describe('startFreshDraft', () => {
  let mockFetch: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    mockFetch = vi.fn().mockResolvedValue(new Response(null, { status: 200 }));
    globalThis.fetch = mockFetch as unknown as typeof fetch;
    connectionStatus.value = 'connected';
    inputMode.value = { type: 'do' };
    selectedScope.value = { kind: 'lucidos' };
    _resetComposeDraftsForTesting();
    _resetComposeSelectionsForTesting();
  });

  afterEach(() => {
    globalThis.fetch = originalFetch;
    connectionStatus.value = 'disconnected';
    focusedThreadId.value = null;
    threadMap.value = new Map();
    inputMode.value = { type: 'do' };
    selectedScope.value = { kind: 'lucidos' };
    _resetComposeDraftsForTesting();
    _resetComposeSelectionsForTesting();
    vi.restoreAllMocks();
  });

  it('opens an unsent draft on the same repo and backend, and empties the old composer', () => {
    const source = makeActiveThread({
      id: 'big', channel: 'claude_code', codingAgentKind: 'external', repoId: 'repo-1',
      codingAgent: 'codex', composeText: 'Add a dark-mode toggle',
    });
    threadMap.value = new Map([['big', source]]);
    focusedThreadId.value = 'big';

    startFreshDraft(source.meta);

    const id = focusedThreadId.value!;
    expect(id).not.toBe('big');
    expect(getDraft(id).text).toBe('Add a dark-mode toggle');
    expect(getDraft(id).mode).toBe('claude_code');
    expect(resolveScope(id)).toEqual({ kind: 'external', repoId: 'repo-1' });
    expect(resolveCodingAgent(id)).toBe('codex');
    expect(getDraft('big').text).toBe('');
    const chatCall = mockFetch.mock.calls.find(([url]) => typeof url === 'string' && url.endsWith('/chat/stream'));
    expect(chatCall, 'moving a draft must not send it').toBeUndefined();
  });

  it('opens an empty compose view on the same repo and backend when nothing was typed', () => {
    const source = makeActiveThread({
      id: 'big', channel: 'claude_code', codingAgentKind: 'external', repoId: 'repo-1', codingAgent: 'codex',
    });
    threadMap.value = new Map([['big', source]]);
    focusedThreadId.value = 'big';

    startFreshDraft(source.meta);

    expect(focusedThreadId.value).toBeNull();
    expect(resolveScope(null)).toEqual({ kind: 'external', repoId: 'repo-1' });
    expect(resolveCodingAgent(null)).toBe('codex');
    const chatCall = mockFetch.mock.calls.find(([url]) => typeof url === 'string' && url.endsWith('/chat/stream'));
    expect(chatCall, 'starting fresh must not send anything').toBeUndefined();
  });

  it('carries the attached images into the new draft', () => {
    const source = makeActiveThread({ id: 'big', channel: 'claude_code', composeText: 'Fix this layout' });
    threadMap.value = new Map([['big', source]]);
    focusedThreadId.value = 'big';
    setDraft('big', { text: 'Fix this layout', image_hashes: ['h1'], mode: null });

    startFreshDraft(source.meta);

    expect(getDraft(focusedThreadId.value!).image_hashes).toEqual(['h1']);
    expect(getDraft('big').image_hashes).toEqual([]);
  });

  it('a second press keeps the moved draft in front', () => {
    const source = makeActiveThread({ id: 'big', channel: 'claude_code', composeText: 'Next' });
    threadMap.value = new Map([['big', source]]);
    focusedThreadId.value = 'big';

    startFreshDraft(source.meta);
    const moved = focusedThreadId.value!;
    startFreshDraft(source.meta);

    expect(focusedThreadId.value).toBe(moved);
    expect(getDraft(moved).text).toBe('Next');
  });

  it('aims an app thread\'s draft at the same app', () => {
    const source = makeActiveThread({
      id: 'big', channel: 'claude_code', codingAgentKind: 'app',
      codingAgentFolder: '/Users/me/workspaces/myws/data/apps/habit-tracker/', composeText: 'Next',
    });
    threadMap.value = new Map([['big', source]]);
    focusedThreadId.value = 'big';

    startFreshDraft(source.meta);

    expect(resolveScope(focusedThreadId.value)).toEqual({ kind: 'app', appId: 'habit-tracker' });
  });

  it('keeps a legacy thread with no backend on Claude Code, whatever was picked last', () => {
    const source = makeActiveThread({ id: 'old', channel: 'claude_code', composeText: 'Next' });
    threadMap.value = new Map([['old', source]]);
    focusedThreadId.value = 'old';
    patchComposeSelection(null, { codingAgent: 'codex' });

    startFreshDraft(source.meta);

    expect(resolveCodingAgent(focusedThreadId.value)).toBe('claude-code');
  });
});

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
    expect(inputMode.value).toEqual({ type: 'coding_agent' });
  });

  it('still auto-discards when text is cleared in the same patch that sets a mode', () => {
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

  it('a peer compose report does not bump updatedAt', () => {
    peerReport('t-1', 'from peer device');
    expect(threadMap.value.get('t-1')!.meta.updatedAt).toBe('2026-05-01T00:00:00.000Z');
  });
});

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

  it('a peer compose report also leaves threadMap untouched', () => {
    const before = threadMap.value;
    peerReport('t-1', 'from peer');
    expect(threadMap.value).toBe(before);
  });
});

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
    selectedScope.value = { kind: 'app', appId: 'last-used-app' };
    const id = ensureFocusedComposeThread();
    expect(getComposeSelectionOverride(id).scope).toEqual({ kind: 'app', appId: 'last-used-app' });
    expect(resolveScope(id)).toEqual({ kind: 'app', appId: 'last-used-app' });
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
    focusThread('existing');
    expect(localStorage.getItem(FOCUSED_THREAD_KEY)).toBe('existing');

    const id = ensureFocusedComposeThread();
    expect(id).toBe('existing');
    expect(localStorage.getItem(FOCUSED_THREAD_KEY)).toBe('existing');
  });
});

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

    const id = ensureFocusedComposeThread();
    updateCompose(id, { text: 'D' });

    await vi.advanceTimersByTimeAsync(300);

    expect(
      seen.filter(isComposePut),
      'compose PUT must NOT fire while POST /threads is still in flight',
    ).toEqual([]);

    resolveCreate!(new Response(null, { status: 201 }));
    await vi.runAllTimersAsync();

    expect(seen.filter(isComposePut)).toHaveLength(1);
    expect(toasts.value.filter((t) => t.type === 'error')).toEqual([]);
  });

  it('forgets the draft\'s pending edit and suppresses a duplicate toast when POST /threads rejects', async () => {
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
    expect(isComposeIntentPending(id)).toBe(true);

    await vi.advanceTimersByTimeAsync(300);

    rejectCreate!(new Error('network down'));
    await vi.runAllTimersAsync();

    expect(isComposeIntentPending(id)).toBe(false);
    const errorToasts = toasts.value.filter((t) => t.type === 'error');
    expect(errorToasts).toHaveLength(1);
    expect(errorToasts[0].message).toMatch(/Failed to start compose/);
  });

  it('skips PUT when discardCompose runs while POST /threads is still in flight', async () => {
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

describe('CC compose pick is per-draft (no cross-thread leak)', () => {
  let mockFetch: ReturnType<typeof vi.fn>;
  let chatBodies: Array<{ use_coding_agent?: boolean; reasoning_effort?: string; cc_model?: string }>;

  beforeEach(() => {
    chatBodies = [];
    mockFetch = vi.fn((url: string, init?: RequestInit) => {
      const method = (init?.method ?? 'GET').toUpperCase();
      if (typeof url === 'string' && url.endsWith('/chat/stream') && method === 'POST') {
        try { chatBodies.push(JSON.parse(init!.body as string)); } catch {  }
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
    expect(chatBodies[0].reasoning_effort).toBe('max');
    expect(getComposeSelectionOverride('cc-1').ccReasoningEffort).toBeUndefined();
  });

  it('does NOT leak the pick onto the next new draft', async () => {
    threadMap.value = new Map<string, ThreadState>().set('cc-A', composingCcThread('cc-A', 'first'));
    focusedThreadId.value = 'cc-A';
    patchComposeSelection('cc-A', { ccReasoningEffort: 'max' });
    await sendCompose('cc-A', { useCodingAgent: true });

    threadMap.value = new Map(threadMap.value).set('cc-B', composingCcThread('cc-B', 'second'));
    focusedThreadId.value = 'cc-B';
    await sendCompose('cc-B', { useCodingAgent: true });

    const ccBodies = chatBodies.filter((b) => b.use_coding_agent);
    expect(ccBodies).toHaveLength(2);
    expect(ccBodies[0].reasoning_effort).toBe('max');
    expect(ccBodies[1].reasoning_effort).toBeUndefined();
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

describe('a pending intent covers the LAST pending write, not the first', () => {
  let resolveFirstPut: (() => void) | null = null;

  beforeEach(() => {
    _resetComposeDraftsForTesting();
    _resetComposeSyncForTesting();
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
    _resetComposeSyncForTesting();
    _resetComposeDraftsForTesting();
    threadMap.value = new Map();
    focusedThreadId.value = null;
    connectionStatus.value = 'disconnected';
  });

  it('stays set when a keystroke re-arms the debounce while the first PUT is in flight', async () => {
    updateCompose('t-1', { text: 'first' });
    await vi.advanceTimersByTimeAsync(300);
    expect(isComposeIntentPending('t-1')).toBe(true);

    updateCompose('t-1', { text: 'first and more' });
    resolveFirstPut?.();
    await vi.advanceTimersByTimeAsync(0);

    expect(isComposeIntentPending('t-1')).toBe(true);
  });
});

describe('undelivered compose drafts are parked and re-sent', () => {
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
    expect(errors[0].key).toBeTruthy();
  });

  it('surfaces a REFUSAL immediately, without parking it', async () => {
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

  function bootSplash(): Response {
    const html = [
      '<!doctype html><html><head>',
      '<meta http-equiv="refresh" content="2">',
      '<meta name="viewport" content="width=device-width, initial-scale=1, viewport-fit=cover">',
      '<meta name="theme-color" content="#0a4ea8">',
      '<link rel="icon" type="image/svg+xml" href="data:image/svg+xml,...">',
      '</head><body><div class="boot-splash">Starting engine</div></body></html>',
    ].join('\n');
    return new Response(html, {
      status: 503,
      headers: {
        'Content-Type': 'text/html; charset=utf-8',
        'Retry-After': '2',
        'x-lucidos-boot-splash': '1',
      },
    });
  }

  it('treats the gateway boot splash as no answer, so the draft is owed a re-send', async () => {
    mockFetch.mockResolvedValue(bootSplash());

    updateCompose('t-1', { text: 'typed while the engine was restarting' });
    await vi.runAllTimersAsync();

    expect(toasts.value.filter((t) => t.type === 'error')).toEqual([]);
    expect(_undeliveredComposeDraftsForTesting()).toEqual(['t-1']);
  });

  it('never puts an HTML body into the card, however the page arrived', async () => {
    mockFetch.mockResolvedValue(new Response(
      '<!doctype html>\n<html><head>\n<meta http-equiv="refresh" content="2">\n</head></html>',
      { status: 502, statusText: 'Bad Gateway', headers: { 'Content-Type': 'text/html' } },
    ));

    updateCompose('t-1', { text: 'against a broken proxy' });
    await vi.runAllTimersAsync();

    const [card] = toasts.value.filter((t) => t.type === 'error');
    expect(card.message).toBe('Compose sync failed: 502 Bad Gateway');
    expect(card.message).not.toMatch(/[<>]/);
    expect(card.message).not.toContain('\n');
  });

  it('re-sends the CURRENT draft on flush, then drains and retracts the card', async () => {
    mockFetch.mockRejectedValue(noAnswer());
    for (const text of ['a', 'ab', 'abc']) {
      updateCompose('t-1', { text });
      await vi.runAllTimersAsync();
    }
    expect(toasts.value.filter((t) => t.type === 'error')).toHaveLength(1);
    const before = composePuts(mockFetch).length;

    mockFetch.mockResolvedValue(new Response(null, { status: 204 }));
    flushUndeliveredComposeDrafts();
    await vi.runAllTimersAsync();

    const sent = composePuts(mockFetch);
    expect(sent.length).toBe(before + 1);
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

    _resetUndeliveredComposeDraftsForTesting();
    const before = composePuts(mockFetch).length;
    flushUndeliveredComposeDrafts();
    await vi.runAllTimersAsync();
    expect(composePuts(mockFetch).length).toBe(before);
  });

  it('a re-push is a delivery attempt, not a new edit', async () => {
    mockFetch.mockRejectedValue(noAnswer());
    updateCompose('t-1', { text: 'owed' });
    await vi.runAllTimersAsync();
    const seq = composeEditSeq('t-1');

    flushUndeliveredComposeDrafts();
    await vi.runAllTimersAsync();

    expect(composeEditSeq('t-1')).toBe(seq);
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
    const before = composePuts(mockFetch).length;
    flushUndeliveredComposeDrafts();
    await vi.runAllTimersAsync();
    const after = composePuts(mockFetch);
    expect(after.length).toBe(before + 1);
    expect(JSON.parse(String((after[after.length - 1][1] as RequestInit).body)).text).toBe('');
  });
});

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
    await vi.advanceTimersByTimeAsync(300);
    expect(resolvers).toHaveLength(1);

    updateCompose('t-1', { text: 'abcd' });
    await vi.advanceTimersByTimeAsync(300);

    expect(resolvers).toHaveLength(1);
    expect(isComposeIntentPending('t-1')).toBe(true);
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

    expect(composePuts()).toEqual(['abc', 'abcde']);
  });

  it('the engine ends up holding the newest text, never an older revision', async () => {
    updateCompose('t-1', { text: 'You can have both' });
    await vi.advanceTimersByTimeAsync(300);
    updateCompose('t-1', { text: 'You can have both from us' });
    await vi.advanceTimersByTimeAsync(300);

    resolvers[0].resolve(new Response(null, { status: 204 }));
    await vi.runAllTimersAsync();
    resolvers[1]?.resolve(new Response(null, { status: 204 }));
    await vi.runAllTimersAsync();

    const applied = composePuts();
    expect(applied[applied.length - 1]).toBe('You can have both from us');
  });

  it('keeps the edit pending across the whole queued window', async () => {
    updateCompose('t-1', { text: 'abc' });
    await vi.advanceTimersByTimeAsync(300);
    updateCompose('t-1', { text: 'abcd' });
    await vi.advanceTimersByTimeAsync(300);

    resolvers[0].resolve(new Response(null, { status: 204 }));
    await vi.advanceTimersByTimeAsync(0);

    expect(isComposeIntentPending('t-1')).toBe(true);

    resolvers[1].resolve(new Response(null, { status: 204 }));
    await vi.runAllTimersAsync();
    expect(isComposeIntentPending('t-1')).toBe(false);
  });

  it('the tab-close flush carries a write queued behind an in-flight one', async () => {
    updateCompose('t-1', { text: 'hello' });
    await vi.advanceTimersByTimeAsync(300);
    updateCompose('t-1', { text: 'hello world' });
    await vi.advanceTimersByTimeAsync(300);

    window.dispatchEvent(new Event('pagehide'));

    const flushed = mockFetch.mock.calls
      .filter(([, init]) => (init as RequestInit | undefined)?.keepalive)
      .map(([, init]) => JSON.parse(String((init as RequestInit).body)).text as string);
    expect(flushed).toEqual(['hello world']);
    const keepalive = mockFetch.mock.calls.find(([, init]) => (init as RequestInit | undefined)?.keepalive);
    expect(new Headers((keepalive?.[1] as RequestInit).headers).get('x-lucidos-restore-generation')).toBe('none');
  });

  it('the tab-close flush still goes out while restore admission is closed', async () => {
    const { installRestoreAdmission } = await import('../../utils/restoreAdmission');
    installRestoreAdmission({ ready: () => false, phase: () => 'awaiting-hello', wait: () => new Promise(() => {}) });
    try {
      updateCompose('t-1', { text: 'typed during a reconnect' });
      window.dispatchEvent(new Event('pagehide'));
      const flushed = mockFetch.mock.calls
        .filter(([, init]) => (init as RequestInit | undefined)?.keepalive)
        .map(([, init]) => JSON.parse(String((init as RequestInit).body)).text as string);
      expect(flushed).toEqual(['typed during a reconnect']);
    } finally {
      installRestoreAdmission({ ready: () => true, phase: () => 'admitted', wait: async () => {} });
    }
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

    expect(composePuts()).toEqual(['abc', 'abcd']);
    expect(_undeliveredComposeDraftsForTesting()).toEqual([]);
    expect(toasts.value.filter((t) => t.type === 'error')).toEqual([]);
  });
});

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
    expect(bodies[0].compose_epoch).toBeUndefined();
    expect(bodies[1].compose_epoch).toBe(7);
    expect(bodies[1].text).toBe('typed while behind a submission');
    expect(toasts.value.filter((t) => t.type === 'error')).toEqual([]);
    expect(_undeliveredComposeDraftsForTesting()).toEqual([]);
    expect(knownComposeEpoch('t-1')).toBe(7);
  });

  it('retries a stale PUT with exact restoration additions after missed SSE', async () => {
    _resetComposeSyncForTesting();
    acceptComposeReport('t-1', { revision: 1, epoch: 1, text: 'saved', imageHashes: ['old'], mode: null, selection: null });
    let calls = 0;
    mockFetch = vi.fn(() => Promise.resolve(++calls === 1
      ? new Response(JSON.stringify({ summary_version: 3, compose_epoch: 2,
        text: 'committed before Stop\n\nqueued', image_hashes: ['server', 'queued'],
        restorations: [{ epoch: 2, text: 'queued', image_hashes: ['queued'] }] }), { status: 412 })
      : new Response(null, { status: 204 })));
    globalThis.fetch = mockFetch as unknown as typeof fetch;
    updateCompose('t-1', { text: 'local replacement', image_hashes: ['local'] });
    await vi.runAllTimersAsync();
    expect(composeBodies()[1]).toMatchObject({ text: 'local replacement\n\nqueued', image_hashes: ['local', 'queued'], compose_epoch: 2 });
    expect(isComposeIntentPending('t-1')).toBe(false);
  });

  it('a reconnect summary rebases additions into a pending local draft', async () => {
    _resetComposeSyncForTesting();
    acceptComposeReport('t-1', { revision: 1, epoch: 1, text: 'saved', imageHashes: [], mode: null, selection: null });
    mockFetch = vi.fn(() => Promise.resolve(new Response(null, { status: 204 })));
    globalThis.fetch = mockFetch as unknown as typeof fetch;
    updateCompose('t-1', { text: 'local replacement' });
    const map = new Map(threadMap.value);
    upsertThread(map, { ...staleComposingSummary('t-1', 'saved\n\nqueued'), state: 'active',
      summary_version: 3, compose_epoch: 2,
      compose_restorations: [{ epoch: 2, text: 'queued', image_hashes: [] }] }, false);
    threadMap.value = map;
    expect(getDraft('t-1').text).toBe('local replacement\n\nqueued');
    await vi.runAllTimersAsync();
    expect(composeBodies().slice(-1)[0].text).toBe('local replacement\n\nqueued');
  });

  it('sends rebased content after an older successful PUT acknowledgement', async () => {
    _resetComposeSyncForTesting();
    acceptComposeReport('t-1', { revision: 1, epoch: 1, text: 'saved', imageHashes: [], mode: null, selection: null });
    let answer!: (response: Response) => void;
    mockFetch = vi.fn().mockImplementationOnce(() => new Promise<Response>(resolve => { answer = resolve; }))
      .mockImplementation(() => Promise.resolve(new Response(null, { status: 204 })));
    globalThis.fetch = mockFetch as unknown as typeof fetch;
    updateCompose('t-1', { text: 'committed edit' });
    await vi.advanceTimersByTimeAsync(300);
    acceptComposeReport('t-1', { revision: 3, epoch: 2, text: 'committed edit\n\nqueued', imageHashes: [], mode: null, selection: null,
      restorations: [{ epoch: 2, text: 'queued', image_hashes: [] }] });
    answer(new Response(JSON.stringify({ summary_version: 2, compose_epoch: 1, text: 'committed edit', image_hashes: [] })));
    await vi.runAllTimersAsync();
    expect(composeBodies()).toHaveLength(2);
    expect(composeBodies()[1]).toMatchObject({ text: 'committed edit\n\nqueued', compose_epoch: 2 });
    expect(getDraft('t-1').text).toBe('committed edit\n\nqueued');
    expect(isComposeIntentPending('t-1')).toBe(false);
  });

  it('parks the draft rather than spinning when the slot keeps being consumed', async () => {
    let epoch = 1;
    mockFetch = vi.fn(() => Promise.resolve(
      new Response(JSON.stringify({ error: 'stale', compose_epoch: epoch++ }), { status: 412 }),
    ));
    globalThis.fetch = mockFetch as unknown as typeof fetch;

    updateCompose('t-1', { text: 'racing a busy peer' });
    await vi.runAllTimersAsync();

    expect(composeBodies()).toHaveLength(3);
    expect(_undeliveredComposeDraftsForTesting()).toEqual(['t-1']);
    expect(toasts.value.filter((t) => t.type === 'error')).toEqual([]);
  });
});

describe('a send leaves the engine holding an empty draft', () => {
  let mockFetch: ReturnType<typeof vi.fn>;

  const composePutTexts = () => mockFetch.mock.calls
    .filter(([url, init]) => String(url).endsWith('/compose') && (init as RequestInit | undefined)?.method === 'PUT')
    .map(([, init]) => JSON.parse(String((init as RequestInit).body)).text as string);
  const lastComposePutText = () => composePutTexts().slice(-1)[0];

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
    unsentMessages.value = new Map();
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

  it('after a send settles, a peer\'s newer draft applies here', async () => {
    threadMap.value = new Map([['t-1', makeThread({ id: 't-1', state: 'composing', composeText: 'from the phone' })]]);

    await sendCompose('t-1', {});
    await vi.runAllTimersAsync();
    expect(isComposeIntentPending('t-1')).toBe(false);

    peerReport('t-1', 'the next draft, typed on the phone');
    expect(getDraft('t-1').text).toBe('the next draft, typed on the phone');
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
    threadMap.value = new Map([['t-1', makeThread({ id: 't-1', state: 'composing', composeText: 'the draft' })]]);
    patchComposeSelection('t-1', { model: 'claude-sonnet-5' });

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
    updateCompose('t-1', { text: 'and one more thing' });
    await vi.runAllTimersAsync();

    const texts = composePutTexts();
    expect(texts[texts.length - 1]).toBe('and one more thing');
  });

  it('an unsent first send clears the engine draft but keeps its picks until a retry is accepted', async () => {
    let chatAnswers = false;
    let revision = 0;
    let storedSelection: unknown = null;
    mockFetch.mockImplementation((url: string, init?: RequestInit) => {
      if (String(url).endsWith('/chat/stream')) {
        if (!chatAnswers) return Promise.reject(new TypeError('Load failed'));
        storedSelection = null;
        return Promise.resolve(chatAccepted());
      }
      if (init?.method === 'PUT') {
        const body = JSON.parse(String(init.body));
        if (body.selection) storedSelection = body.selection;
        revision += 1;
        return Promise.resolve(new Response(JSON.stringify({
          summary_version: revision, compose_epoch: 0, text: body.text, image_hashes: body.image_hashes ?? [], selection: storedSelection,
        }), { status: 200 }));
      }
      return Promise.resolve(chatAccepted());
    });
    threadMap.value = new Map([['t-1', makeThread({ id: 't-1', state: 'composing', composeText: 'the draft' })]]);
    patchComposeSelection('t-1', { model: 'claude-sonnet-5' });

    const sending = sendCompose('t-1', {});
    await vi.runAllTimersAsync();
    await sending;

    const sent = composePutTexts();
    expect(sent[sent.length - 1]).toBe('');
    expect(getComposeSelectionOverride('t-1').model).toBe('claude-sonnet-5');

    chatAnswers = true;
    const [eventId] = [...unsentMessages.value.keys()];
    expect(await retryUnsentMessage(eventId)).toBe('sent');
    await vi.runAllTimersAsync();

    const texts = composePutTexts();
    expect(texts[texts.length - 1]).toBe('');
    expect(getComposeSelectionOverride('t-1').model).toBeUndefined();
  });

  it('a resync while a first send sits unsent does not restage the pre-send draft', async () => {
    let chatAnswers = false;
    mockFetch.mockImplementation((url: string) => {
      if (String(url).endsWith('/chat/stream') && !chatAnswers) {
        return Promise.reject(new TypeError('Load failed'));
      }
      return Promise.resolve(chatAccepted());
    });
    threadMap.value = new Map([['t-1', makeThread({ id: 't-1', state: 'composing', composeText: 'the draft, typed in full' })]]);

    const sending = sendCompose('t-1', {});
    await vi.runAllTimersAsync();
    await sending;
    vi.advanceTimersByTime(1000);

    const map = new Map(threadMap.value);
    upsertThread(map, staleComposingSummary('t-1', 'the draft, ty'), false);
    threadMap.value = map;

    expect(threadMap.value.get('t-1')!.meta.state).toBe('active');
    expect(getDraft('t-1').text).toBe('');

    chatAnswers = true;
    const [eventId] = [...unsentMessages.value.keys()];
    expect(await retryUnsentMessage(eventId)).toBe('sent');
    await vi.runAllTimersAsync();

    expect(composePutTexts()).not.toContain('the draft, ty');
    const texts = composePutTexts();
    expect(texts[texts.length - 1]).toBe('');
  });

  it('a refused retry of an unsent first send rolls the draft back with its picks', async () => {
    let chatAnswer: 'none' | 'refuse' = 'none';
    mockFetch.mockImplementation((url: string) => {
      if (String(url).endsWith('/chat/stream')) {
        if (chatAnswer === 'none') return Promise.reject(new TypeError('Load failed'));
        return Promise.resolve(new Response(JSON.stringify({ error: 'locked' }), { status: 409 }));
      }
      return Promise.resolve(chatAccepted());
    });
    threadMap.value = new Map([['t-1', makeThread({ id: 't-1', state: 'composing', composeText: 'the draft' })]]);
    patchComposeSelection('t-1', { model: 'claude-sonnet-5' });

    const sending = sendCompose('t-1', {});
    await vi.runAllTimersAsync();
    await sending;
    chatAnswer = 'refuse';
    const [eventId] = [...unsentMessages.value.keys()];
    expect(await retryUnsentMessage(eventId)).toBe('dropped');
    await vi.runAllTimersAsync();

    expect(threadMap.value.get('t-1')!.meta.state).toBe('composing');
    expect(getDraft('t-1').text).toBe('the draft');
    expect(getComposeSelectionOverride('t-1').model).toBe('claude-sonnet-5');
    expect(lastComposePutText()).toBe('the draft');
  });

});

describe('a send the engine refuses keeps the typed text', () => {
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
    mockFetch = vi.fn((url: unknown) => Promise.resolve(String(url).endsWith('/chat/stream')
      ? new Response(JSON.stringify({ error: 'the engine said no' }), { status: 500 })
      : new Response(JSON.stringify({ event_id: 'e-1' }), { status: 200 })));
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

  it('sendCompose restores the draft and leaves the thread composing', async () => {
    threadMap.value = new Map([['t-1', makeThread({ id: 't-1', state: 'composing', composeText: 'the draft' })]]);

    await sendCompose('t-1', {}).catch(() => {});
    await vi.runAllTimersAsync();

    expect(getDraft('t-1').text).toBe('the draft');
    expect(threadMap.value.get('t-1')?.meta.state).toBe('composing');
    expect(composePutTexts()).not.toContain('');
  });

  it.each([
    { kind: 'first-send', mode: null },
    { kind: 'raw-new' },
  ] as const)('returns an overtaken $kind before the competing send is recorded', async settlement => {
    const thread = makeActiveThread({ id: 't-1', messageCount: 0 });
    thread.unsentMessageSeqs = new Map([['e-1', -2]]);
    threadMap.value = new Map([['t-1', thread]]);
    unsentMessages.value = new Map([['e-1', {
      threadId: 't-1',
      body: { message: 'held text', mode: 'human', event_id: 'e-1', thread_id: 't-1' },
      failedRetries: 0,
      settlement,
      refusal: { reason: 'hook_input_overtaken', message: 'Another message reached this thread first.' },
    }]]);

    await cancelUnsentHookInput('e-1');
    expect(unsentMessages.value.has('e-1')).toBe(false);
    expect(getDraft('t-1').text).toBe('held text');
    expect(threadMap.value.get('t-1')?.meta.state).toBe('active');
  });

  it('sendCompose stores the restored text when Send beat the draft write', async () => {
    threadMap.value = new Map([['t-1', makeThread({ id: 't-1', state: 'composing' })]]);
    updateCompose('t-1', { text: 'typed then sent' });

    await sendCompose('t-1', {}).catch(() => {});
    await vi.runAllTimersAsync();

    expect(getDraft('t-1').text).toBe('typed then sent');
    const texts = composePutTexts();
    expect(texts[texts.length - 1]).toBe('typed then sent');
  });

  it('sendCompose keeps new typing and puts the refused text after it', async () => {
    threadMap.value = new Map([['t-1', makeThread({ id: 't-1', state: 'composing', composeText: 'the draft' })]]);

    const send = sendCompose('t-1', {}).catch(() => {});
    updateCompose('t-1', { text: 'typed meanwhile' });
    await send;
    await vi.runAllTimersAsync();

    expect(getDraft('t-1').text).toBe('typed meanwhile\n\nthe draft');
    expect(threadMap.value.get('t-1')?.meta.state).toBe('composing');
  });

  it('sendCompose puts the refused text back over a blank typed meanwhile, caret at the end', async () => {
    threadMap.value = new Map([['t-1', makeThread({ id: 't-1', state: 'composing', composeText: 'the draft' })]]);

    const send = sendCompose('t-1', {}).catch(() => {});
    updateCompose('t-1', { text: ' ' });
    const seqBefore = promptCaretRequest.value.seq;
    await send;
    await vi.runAllTimersAsync();

    expect(getDraft('t-1').text).toBe('the draft');
    expect(promptCaretRequest.value.seq).toBe(seqBefore + 1);
  });

  it('sendFollowup puts the text back into the draft', async () => {
    threadMap.value = new Map([['t-1', makeActiveThread({ id: 't-1', composeText: 'a follow-up' })]]);

    await sendFollowup('t-1', 'a follow-up');
    await vi.runAllTimersAsync();

    expect(getDraft('t-1').text).toBe('a follow-up');
    const texts = composePutTexts();
    expect(texts[texts.length - 1]).toBe('a follow-up');
  });

  it('sendFollowup keeps new typing and puts the refused text after it', async () => {
    threadMap.value = new Map([['t-1', makeActiveThread({ id: 't-1', composeText: 'a follow-up' })]]);

    const send = sendFollowup('t-1', 'a follow-up');
    updateCompose('t-1', { text: 'typed meanwhile' });
    await send;
    await vi.runAllTimersAsync();

    expect(getDraft('t-1').text).toBe('typed meanwhile\n\na follow-up');
  });
});

describe('sendCompose waits for the thread row before the chat POST', () => {
  let mockFetch: ReturnType<typeof vi.fn>;
  let releaseThreadStart: ((value: Response) => void) | null;

  const chatCalls = () => mockFetch.mock.calls.filter(([url]) =>
    typeof url === 'string' && url.endsWith('/chat/stream'));

  beforeEach(() => {
    releaseThreadStart = null;
    mockFetch = vi.fn().mockImplementation((url: unknown, init?: RequestInit) => {
      if (typeof url === 'string' && url.endsWith('/threads') && init?.method === 'POST') {
        return new Promise<Response>((resolve) => { releaseThreadStart = resolve; });
      }
      return Promise.resolve(chatAccepted());
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
    confirmState.value = { visible: false, message: '', okLabel: 'Delete' };
    focusedPane.value = 'thread';
    _resetComposeDraftsForTesting();
    _resetComposeSelectionsForTesting();
    vi.restoreAllMocks();
  });

  it('holds the chat POST until POST /threads settles, then sends it', async () => {
    const started = startSetupInterview();
    await Promise.resolve();
    await Promise.resolve();
    expect(chatCalls(), 'chat POST fired before the thread row existed').toHaveLength(0);

    releaseThreadStart!(new Response(null, { status: 200 }));
    await expect(started).resolves.toBe(true);
    expect(chatCalls(), 'chat POST never fired after the thread row landed').toHaveLength(1);
  });

  it('shows the send as in flight while POST /threads is held', async () => {
    const started = startSetupInterview();
    await Promise.resolve();
    await Promise.resolve();
    const thread = threadMap.value.get(focusedThreadId.value!)!;
    expect(thread.pendingUserMessages, 'no optimistic row while the thread starts').toHaveLength(1);
    expect(effectiveThreadStatus(thread)).toBe('running');

    handleThreadEvent({
      thread_id: thread.meta.id, seq: 1, created: '2026-01-01T00:00:00Z',
      event: { type: 'ThreadStarted' },
      aggregate: {
        ...thread.meta, threadId: thread.meta.id, state: 'composing',
        summaryVersion: 1, lastActivity: '', isSaved: false,
      },
    });
    expect(thread.meta.state).toBe('active');
    expect(composeViewActive.value).toBe(false);

    releaseThreadStart!(new Response(null, { status: 200 }));
    await expect(started).resolves.toBe(true);
  });

  it('reports a failed thread start instead of silently sending nothing', async () => {
    const started = startSetupInterview();
    await Promise.resolve();
    releaseThreadStart!(new Response(null, { status: 500 }));

    await expect(started).resolves.toBe(false);
    expect(chatCalls(), 'chat POST fired despite the thread row failing').toHaveLength(0);
    expect(toasts.value.filter((t) => t.type === 'error').length).toBeGreaterThan(0);
  });

  it('restores compose view when the first message is refused after thread creation', async () => {
    const started = startSetupInterview();
    const id = focusedThreadId.value!;
    expect(composeViewActive.value).toBe(false);
    mockFetch.mockResolvedValue(new Response(JSON.stringify({ error: 'Send refused' }), { status: 403 }));
    releaseThreadStart!(new Response(null, { status: 200 }));

    await expect(started).resolves.toBe(false);
    expect(threadMap.value.get(id)!.meta.state).toBe('composing');
    expect(composeViewActive.value).toBe(true);
    expect(getDraft(id).text.length).toBeGreaterThan(0);
  });

  it('surfaces the thread pane, before the send rather than after it', async () => {
    focusedPane.value = 'content';
    const started = startSetupInterview();
    await Promise.resolve();
    await Promise.resolve();
    expect(focusedPane.value, 'thread pane still hidden while the send is in flight')
      .toBe('thread');

    releaseThreadStart!(new Response(null, { status: 200 }));
    await expect(started).resolves.toBe(true);
  });

  it('sends from a new draft, leaving a draft in progress untouched', async () => {
    const map = new Map<string, ThreadState>();
    map.set('t-1', makeThread({ state: 'composing', composeText: 'my own idea' }));
    threadMap.value = map;
    focusedThreadId.value = 't-1';
    focusedPane.value = 'content';

    const started = startSetupInterview();
    expect(confirmState.value.visible, 'still asking about a draft it does not touch').toBe(false);

    releaseThreadStart!(new Response(null, { status: 200 }));
    await expect(started).resolves.toBe(true);
    expect(focusedThreadId.value).not.toBe('t-1');
    expect(getDraft('t-1').text).toBe('my own idea');
  });
});

describe('a suggestion never lands on an already-sent thread', () => {
  let mockFetch: ReturnType<typeof vi.fn>;

  const chatBodies = () => mockFetch.mock.calls
    .filter(([url]) => typeof url === 'string' && url.endsWith('/chat/stream'))
    .map(([, init]) => JSON.parse((init as RequestInit).body as string));

  beforeEach(() => {
    mockFetch = vi.fn(() => Promise.resolve(chatAccepted()));
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

  it('seedSuggestion prefills a fresh draft rather than the open thread', () => {
    const draftId = seedSuggestion('summarize my week');

    expect(draftId).not.toBe('cc-1');
    expect(focusedThreadId.value).toBe(draftId);
    expect(getDraft(draftId).text).toBe('summarize my week');
    expect(getDraft('cc-1').text).toBe('');
  });

  it('does not stop to ask, whatever the focused thread is', () => {
    seedSuggestion('summarize my week');
    expect(confirmState.value.visible, 'asked about a thread it was never going to touch').toBe(false);
  });

  it('leaves a half-typed follow-up on the open thread alone', () => {
    setDraft('cc-1', { text: 'and also rename the button', image_hashes: ['h1'], mode: null });

    seedSuggestion('summarize my week');

    expect(confirmState.value.visible).toBe(false);
    expect(getDraft('cc-1').text).toBe('and also rename the button');
    expect(getDraft('cc-1').image_hashes).toEqual(['h1']);
  });
});

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
    mockFetch.mockClear();

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

    expect(composeModes()).not.toEqual([]);
    expect(composeModes().every((m) => m === 'claude_code')).toBe(true);
  });
});

describe('a compose backend pick is per-draft', () => {
  let chatBodies: Array<{ provider?: string; model?: string }>;

  beforeEach(() => {
    chatBodies = [];
    globalThis.fetch = vi.fn((url: string, init?: RequestInit) => {
      const method = (init?.method ?? 'GET').toUpperCase();
      if (typeof url === 'string' && url.endsWith('/chat/stream') && method === 'POST') {
        chatBodies.push(JSON.parse(init!.body as string));
        return Promise.resolve(new Response(JSON.stringify({ event_id: 'evt' }), {
          status: 200, headers: { 'Content-Type': 'application/json' },
        }));
      }
      return Promise.resolve(new Response(null, { status: 200 }));
    }) as unknown as typeof fetch;
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

  function composingChat(id: string, text: string): ThreadState {
    return makeThread({ id, state: 'composing', channel: 'chat', composeText: text, composeMode: 'lucidos' });
  }

  it("sends this draft's backend, and never another draft's", async () => {
    threadMap.value = new Map<string, ThreadState>()
      .set('d-A', composingChat('d-A', 'first'))
      .set('d-B', composingChat('d-B', 'second'));
    patchComposeSelection('d-A', { model: 'claude-opus-5-6', provider: 'anthropic' });

    expect(resolveProvider('d-B')).toBeNull();

    await sendCompose('d-A', { useCodingAgent: false });
    await sendCompose('d-B', { useCodingAgent: false });

    expect(chatBodies).toHaveLength(2);
    expect(chatBodies[0]).toMatchObject({ model: 'claude-opus-5-6', provider: 'anthropic' });
    expect(chatBodies[1].provider).toBeUndefined();
  });
});

describe('a thread start that gets no answer keeps the draft', () => {
  const noAnswer = () => new DOMException('signal timed out', 'TimeoutError');
  const isStart = (url: unknown, init?: RequestInit) =>
    String(url).endsWith('/threads') && init?.method === 'POST';

  let mockFetch: ReturnType<typeof vi.fn>;
  let startAnswers: Array<() => Promise<Response>>;

  beforeEach(() => {
    connectionStatus.value = 'connected';
    focusedThreadId.value = null;
    threadMap.value = new Map();
    _resetComposeDraftsForTesting();
    _resetUndeliveredComposeDraftsForTesting();
    _resetThreadNavForTesting();
    toasts.value = [];
    startAnswers = [];
    mockFetch = vi.fn((url: unknown, init?: RequestInit) => {
      if (isStart(url, init)) {
        const next = startAnswers.shift();
        return next ? next() : Promise.resolve(new Response(null, { status: 200 }));
      }
      return Promise.resolve(new Response(null, { status: 204 }));
    });
    globalThis.fetch = mockFetch as unknown as typeof fetch;
  });

  afterEach(() => {
    globalThis.fetch = originalFetch;
    connectionStatus.value = 'disconnected';
    focusedThreadId.value = null;
    threadMap.value = new Map();
    _resetComposeDraftsForTesting();
    _resetUndeliveredComposeDraftsForTesting();
    _resetThreadNavForTesting();
    toasts.value = [];
    vi.restoreAllMocks();
  });

  const starts = () => mockFetch.mock.calls.filter(([url, init]) => isStart(url, init as RequestInit));
  const settle = () => new Promise((r) => setTimeout(r, 0));

  it('keeps the row, the focus and the draft, and owes the start', async () => {
    startAnswers.push(() => Promise.reject(noAnswer()));
    const id = ensureFocusedComposeThread();
    updateCompose(id, { text: 'typed while the engine was down' });
    await settle();

    expect(threadMap.value.has(id)).toBe(true);
    expect(focusedThreadId.value).toBe(id);
    expect(getDraft(id).text).toBe('typed while the engine was down');
    expect(isThreadStartPending(id)).toBe(true);
    expect(_undeliveredComposeDraftsForTesting()).toContain(id);
    expect(toasts.value.some((t) => t.message.includes('Failed to start compose'))).toBe(false);
  });

  it('re-sends an owed start to whoever needs the row next', async () => {
    startAnswers.push(() => Promise.reject(noAnswer()));
    const id = ensureFocusedComposeThread();
    await settle();
    expect(starts()).toHaveLength(1);

    await awaitThreadStarted(id);

    expect(starts()).toHaveLength(2);
    expect(isThreadStartPending(id)).toBe(false);
    expect(threadMap.value.has(id)).toBe(true);
  });

  it('re-sends the start on the reconnect flush, then delivers the draft', async () => {
    startAnswers.push(() => Promise.reject(noAnswer()));
    const id = ensureFocusedComposeThread();
    updateCompose(id, { text: 'kept' });
    await settle();

    flushUndeliveredComposeDrafts();
    await vi.waitFor(() => expect(_undeliveredComposeDraftsForTesting()).toEqual([]));

    expect(starts()).toHaveLength(2);
    const puts = mockFetch.mock.calls.filter(([url, init]) =>
      String(url).endsWith('/compose') && (init as RequestInit).method === 'PUT');
    expect(puts.length).toBeGreaterThan(0);
  });

  it('still rolls back a start the engine refused', async () => {
    startAnswers.push(() => Promise.resolve(new Response(JSON.stringify({ error: 'mode conflict' }), { status: 409 })));
    const id = ensureFocusedComposeThread();
    await settle();

    expect(threadMap.value.has(id)).toBe(false);
    expect(focusedThreadId.value).toBeNull();
    expect(isThreadStartPending(id)).toBe(false);
    expect(toasts.value.some((t) => t.message.includes('mode conflict'))).toBe(true);
  });

  it('a Send that gets no answer for the owed start keeps the draft and its row', async () => {
    startAnswers.push(() => Promise.reject(noAnswer()), () => Promise.reject(noAnswer()));
    const id = ensureFocusedComposeThread();
    updateCompose(id, { text: 'send me' });
    await settle();

    await expect(sendCompose(id, { useCodingAgent: false })).rejects.toBeTruthy();

    expect(threadMap.value.has(id)).toBe(true);
    expect(getDraft(id).text).toBe('send me');
    expect(isThreadStartPending(id)).toBe(true);
  });

  it('a re-sent start the engine refuses still says so and drops the nav entry and focus', async () => {
    pushThreadNavState({ type: 'thread', id: 'prior-thread' });
    startAnswers.push(
      () => Promise.reject(noAnswer()),
      () => Promise.resolve(new Response(JSON.stringify({ error: 'mode conflict' }), { status: 409 })),
    );
    const id = ensureFocusedComposeThread();
    await settle();
    expect(threadMap.value.has(id)).toBe(true);

    await expect(awaitThreadStarted(id)).rejects.toBeTruthy();

    expect(threadMap.value.has(id)).toBe(false);
    expect(focusedThreadId.value).toBeNull();
    expect(_threadNavStateForTesting().stack).toEqual([{ type: 'thread', id: 'prior-thread' }]);
    expect(toasts.value.some((t) => t.message.includes('mode conflict'))).toBe(true);
  });

  it('discards an owed draft locally, with no DELETE to an engine that never held it', async () => {
    startAnswers.push(() => Promise.reject(noAnswer()));
    const id = ensureFocusedComposeThread();
    await settle();

    await discardCompose(id);

    expect(threadMap.value.has(id)).toBe(false);
    expect(isThreadStartPending(id)).toBe(false);
    expect(mockFetch.mock.calls.some(([, init]) => (init as RequestInit | undefined)?.method === 'DELETE')).toBe(false);
  });
});


describe('remembered composer coding agent', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    _resetComposeSelectionsForTesting();
    _resetUndeliveredComposeDraftsForTesting();
    focusedThreadId.value = null;
    selectedCodingAgent.value = 'claude-code';
    preferences.value = { status: 'loaded', data: {} };
    threadMap.value = new Map([
      ['legacy', makeThread({ id: 'legacy', state: 'composing' })],
      ['explicit', makeThread({ id: 'explicit', state: 'composing' })],
      ['started', makeThread({ id: 'started', codingAgent: 'claude-code' })],
    ]);
    patchComposeSelection('explicit', { codingAgent: 'codex' });
    globalThis.fetch = vi.fn(async () => new Response('{}', { status: 200 }));
  });

  afterEach(() => {
    _resetUndeliveredComposeDraftsForTesting();
    _resetComposeSelectionsForTesting();
    vi.useRealTimers();
    globalThis.fetch = originalFetch;
    selectedCodingAgent.value = 'claude-code';
  });

  it('remembers the picked agent while preserving implicit and explicit existing drafts', async () => {
    await selectComposeCodingAgent(null, 'codex');
    expect(selectedCodingAgent.value).toBe('codex');
    expect(preferences.value).toMatchObject({ data: { coding_agent_default: 'codex' } });
    expect(globalThis.fetch).toHaveBeenCalledWith(expect.stringContaining('/preferences'), expect.objectContaining({
      method: 'PUT', body: JSON.stringify({ value: 'codex' }),
    }));
    expect(resolveCodingAgent('legacy')).toBe('claude-code');
    expect(resolveCodingAgent('explicit')).toBe('codex');
    expect(threadMap.value.get('started')?.meta.codingAgent).toBe('claude-code');
    const id = ensureFocusedComposeThread();
    expect(getComposeSelectionOverride(id).codingAgent).toBe('codex');
    _resetComposeSelectionsForTesting();
    expect(resolveCodingAgent(null)).toBe('codex');
  });

  it('preserves the inherited agent when a peer preference refresh arrives during an edit', () => {
    updateCompose('legacy', { text: 'new peer text' });
    applyCodingAgentDefault('codex');
    expect(resolveCodingAgent('legacy')).toBe('claude-code');
    expect(resolveCodingAgent('explicit')).toBe('codex');
    expect(resolveCodingAgent(null)).toBe('codex');
    expect(isComposeIntentPending('legacy')).toBe(true);
  });

  it('does not send content writes for unrelated drafts when remembering a pick', async () => {
    await selectComposeCodingAgent(null, 'codex');
    await vi.advanceTimersByTimeAsync(1000);
    expect(vi.mocked(globalThis.fetch).mock.calls.some(([url]) => String(url).includes('/compose'))).toBe(false);
  });

  it('accepts a server agent pin during startup without replacing pending text', () => {
    preferences.value = { status: 'loading' };
    updateCompose('legacy', { text: 'new peer text' });
    acceptComposeReport('legacy', {
      revision: 2, epoch: 0, text: 'older server text', imageHashes: [], mode: 'claude_code',
      selection: { codingAgent: 'codex' },
    });
    expect(resolveCodingAgent('legacy')).toBe('codex');
    expect(getDraft('legacy')?.text).toBe('new peer text');
    patchComposeSelection('legacy', { codingAgent: 'claude-code' });
    acceptComposeReport('legacy', {
      revision: 3, epoch: 0, text: 'older server text', imageHashes: [], mode: 'claude_code',
      selection: { codingAgent: 'codex' },
    });
    expect(resolveCodingAgent('legacy')).toBe('claude-code');
  });

  it('does not persist the boot fallback before the saved preference is known', () => {
    preferences.value = { status: 'loading' };
    const id = ensureFocusedComposeThread();
    expect(getComposeSelectionOverride(id).codingAgent).toBeUndefined();
    selectedCodingAgent.value = 'codex';
    expect(resolveCodingAgent(id)).toBe('codex');
  });

  it('updates the focused draft as well as the next composer', async () => {
    await selectComposeCodingAgent('legacy', 'codex');
    expect(resolveCodingAgent('legacy')).toBe('codex');
    expect(selectedCodingAgent.value).toBe('codex');
  });
});
