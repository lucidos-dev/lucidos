import { describe, it, expect, beforeEach, vi } from 'vitest';

vi.hoisted(() => {
  const storage = new Map<string, string>();
  (globalThis as any).localStorage = {
    getItem: (k: string) => storage.get(k) ?? null,
    setItem: (k: string, v: string) => storage.set(k, v),
    removeItem: (k: string) => storage.delete(k),
    clear: () => storage.clear(),
    get length() { return storage.size; },
    key: (_i: number) => null,
  };
  if (typeof globalThis.document === 'undefined') {
    (globalThis as any).document = {};
  }
  if (!(globalThis.document as any).querySelector) {
    (globalThis.document as any).querySelector = () => null;
  }
  if (!(globalThis.document as any).querySelectorAll) {
    (globalThis.document as any).querySelectorAll = () => [];
  }
  if (typeof globalThis.requestAnimationFrame === 'undefined') {
    (globalThis as any).requestAnimationFrame = (cb: any) => { cb(); return 0; };
  }
  if (typeof globalThis.crypto === 'undefined' || !(globalThis.crypto as any).randomUUID) {
    (globalThis as any).crypto = {
      randomUUID: () => 'test-uuid-' + Math.random().toString(36).slice(2),
    };
  }
});

vi.mock('../../api/client', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../api/client')>()),
  API_BASE: '',
  submitChat: vi.fn().mockResolvedValue({ event_id: 'srv-evt' }),
  cancelChat: vi.fn(),
  stopClaudeCode: vi.fn(),
  putComposeOnThread: vi.fn().mockResolvedValue({ status: 'applied' }),
  ensureThreadStarted: vi.fn().mockResolvedValue(undefined),
  deleteThread: vi.fn().mockResolvedValue(undefined),
}));

vi.mock('./thread-navigation', () => ({
  pushThreadNavState: vi.fn(),
}));

vi.mock('../../components/chat/scrollState', () => ({
  followSentMessage: vi.fn(),
  stopFollowingBottom: vi.fn(),
}));

vi.mock('./thread-loading', () => ({
  refreshThreadEvents: vi.fn().mockResolvedValue(true),
}));

vi.mock('./devices', () => ({
  getDeviceId: () => 'device-test',
  pendingDeviceRegistration: vi.fn(),
}));

vi.mock('../../utils/platform', () => ({
  isTauri: () => false,
  isIOS: () => false,
}));

import {
  focusedThreadId,
  threadMap,
  selectedScope,
  selectedCodingAgent,
  connectionStatus,
  repositories,
  type Scope,
} from '../store';
import { composeDraftContextName, LUCIDOS_SOURCE_REPO_NAME } from '../composeDestination';
import { threadContextName } from '../../components/drawer/threadRowInfo';
import { sendMessage } from './chat';
import { sendCompose } from './compose';
import { patchComposeSelection, _resetComposeSelectionsForTesting } from '../composeSelections';
import { setDraft, _resetComposeDraftsForTesting } from '../composeDrafts';
import { submitChat } from '../../api/client';
import type { ChatRequestBody } from '../../api/types';
import type { ThreadMeta } from '../thread-events';

const mockedSubmitChat = vi.mocked(submitChat);

beforeEach(() => {
  threadMap.value = new Map();
  focusedThreadId.value = null;
  selectedScope.value = { kind: 'lucidos' };
  selectedCodingAgent.value = 'claude-code';
  connectionStatus.value = 'connected';
  mockedSubmitChat.mockClear();
  _resetComposeDraftsForTesting();
  _resetComposeSelectionsForTesting();
});

function lastBody(): ChatRequestBody {
  expect(mockedSubmitChat).toHaveBeenCalledTimes(1);
  return mockedSubmitChat.mock.calls[0][0];
}

function makeMeta(id: string, overrides: Partial<ThreadMeta>): ThreadMeta {
  return {
    id,
    title: 't',
    channel: 'claude_code',
    initiator: 'user',
    saved: false,
    createdAt: '',
    updatedAt: '',
    status: 'idle',
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
    liveEventWaitCount: 0,
    liveEventWaits: [],
    ...overrides,
  };
}

function putThread(id: string, metaOverrides: Partial<ThreadMeta>): void {
  threadMap.value = new Map([[id, {
    meta: makeMeta(id, metaOverrides),
    events: new Map(),
    streamingBuffer: '',
    eventsLoaded: true,
    eventsLoadFailed: false,
    lastDbSeq: 0,
    pendingUserMessages: [],
  } as any]]);
}

describe('sendMessage scope selection (regression: dropdown ignored)', () => {
  it('new CC thread (no draft, no focus) sends folder from external scope', async () => {
    selectedScope.value = { kind: 'external', repoId: 'external-repo-uuid' };

    await sendMessage('fix the merge conflict', undefined, { useCodingAgent: true });

    const body = lastBody();
    expect(body.use_coding_agent).toBe(true);
    expect(body.folder).toBe('external-repo-uuid');
    expect(body.repo_id).toBeUndefined();
  });

  it('new CC thread with Lucidos scope omits folder (default Lucidos)', async () => {
    selectedScope.value = { kind: 'lucidos' };

    await sendMessage('hello', undefined, { useCodingAgent: true });

    const body = lastBody();
    expect(body.use_coding_agent).toBe(true);
    expect(body.folder).toBeUndefined();
    expect(body.repo_id).toBeUndefined();
  });

  it('new CC thread with app scope sends folder=data/apps/<id>', async () => {
    selectedScope.value = { kind: 'app', appId: 'habit-tracker' };

    await sendMessage('fix the chart bug', undefined, { useCodingAgent: true });

    const body = lastBody();
    expect(body.use_coding_agent).toBe(true);
    expect(body.folder).toBe('data/apps/habit-tracker');
    expect(body.repo_id).toBeUndefined();
  });

  it('follow-up on active CC thread uses meta.repoId, ignores selectedScope', async () => {
    const tid = 'existing-thread';
    focusedThreadId.value = tid;
    putThread(tid, { state: 'active', channel: 'claude_code', messageCount: 1, repoId: 'repo-A-uuid' });
    selectedScope.value = { kind: 'external', repoId: 'repo-B-uuid' };

    await sendMessage('follow up', undefined, { useCodingAgent: true });

    const body = lastBody();
    expect(body.use_coding_agent).toBe(true);
    expect(body.repo_id).toBe('repo-A-uuid');
    expect(body.folder).toBeUndefined();
  });

  it('follow-up on active CC thread bound to default Lucidos (no meta.repoId) ignores dropdown', async () => {
    const tid = 'lucidos-thread';
    focusedThreadId.value = tid;
    putThread(tid, { state: 'active', channel: 'claude_code', messageCount: 1 });
    selectedScope.value = { kind: 'external', repoId: 'repo-B-uuid' };

    await sendMessage('follow up', undefined, { useCodingAgent: true });

    const body = lastBody();
    expect(body.use_coding_agent).toBe(true);
    expect(body.repo_id).toBeUndefined();
    expect(body.folder).toBeUndefined();
  });

  it('follow-up on active app CC thread re-sends folder from meta.codingAgentFolder', async () => {
    const tid = 'app-thread';
    focusedThreadId.value = tid;
    putThread(tid, {
      state: 'active',
      channel: 'claude_code',
      messageCount: 1,
      codingAgentKind: 'app',
      codingAgentFolder: '/some/workspace/data/apps/habit-tracker',
    });
    selectedScope.value = { kind: 'lucidos' };

    await sendMessage('keep going', undefined, { useCodingAgent: true });

    const body = lastBody();
    expect(body.use_coding_agent).toBe(true);
    expect(body.folder).toBe('data/apps/habit-tracker');
    expect(body.repo_id).toBeUndefined();
  });
});

describe('sendCompose carries dropdown scope through to chat body (real flow)', () => {
  it('promoting a CC draft to active sends folder from external scope', async () => {
    const draftId = 'draft-thread';
    focusedThreadId.value = draftId;
    putThread(draftId, { state: 'composing', channel: 'claude_code' });
    setDraft(draftId, { text: 'fix it', image_hashes: [], mode: 'claude_code' });
    // The draft carries its OWN scope (set by the picker via applyDestination /
    // eager-seeded at creation) — the send resolves that, not the shared
    // selectedScope. selectedScope is deliberately different to prove it.
    patchComposeSelection(draftId, { scope: { kind: 'external', repoId: 'external-repo-uuid' } });
    selectedScope.value = { kind: 'lucidos' };

    await sendCompose(draftId, { useCodingAgent: true });

    const body = lastBody();
    expect(body.use_coding_agent).toBe(true);
    // sendCompose flips composing→active BEFORE delegating, so the chat path
    // now treats this as a follow-up and reads meta.repoId — which compose
    // bound from the external scope.
    expect(body.repo_id).toBe('external-repo-uuid');
  });

  it('promoting an app-scope CC draft sends folder=data/apps/<id>', async () => {
    const draftId = 'app-draft';
    focusedThreadId.value = draftId;
    putThread(draftId, { state: 'composing', channel: 'claude_code' });
    setDraft(draftId, { text: 'fix it', image_hashes: [], mode: 'claude_code' });
    // Draft's own scope override — resolved at send, not the shared selectedScope.
    patchComposeSelection(draftId, { scope: { kind: 'app', appId: 'habit-tracker' } });
    selectedScope.value = { kind: 'lucidos' };

    await sendCompose(draftId, { useCodingAgent: true });

    const body = lastBody();
    expect(body.use_coding_agent).toBe(true);
    expect(body.folder).toBe('data/apps/habit-tracker');
    expect(body.repo_id).toBeUndefined();
  });

  it('promoting a Lucidos-source Codex draft sends coding_agent=codex', async () => {
    const draftId = 'codex-draft';
    focusedThreadId.value = draftId;
    putThread(draftId, { state: 'composing', channel: 'claude_code' });
    setDraft(draftId, { text: 'fix it', image_hashes: [], mode: 'claude_code' });
    selectedScope.value = { kind: 'lucidos' };
    selectedCodingAgent.value = 'codex';

    await sendCompose(draftId, { useCodingAgent: true });

    const body = lastBody();
    expect(body.use_coding_agent).toBe(true);
    expect(body.coding_agent).toBe('codex');
    expect(body.folder).toBeUndefined();
    expect(body.repo_id).toBeUndefined();
  });

  it('promoting a non-CC draft does not bind a repo even if dropdown has one', async () => {
    const draftId = 'draft-chat';
    focusedThreadId.value = draftId;
    putThread(draftId, { state: 'composing', channel: 'chat' });
    setDraft(draftId, { text: 'hi', image_hashes: [], mode: null });
    selectedScope.value = { kind: 'external', repoId: 'external-repo-uuid' };

    await sendCompose(draftId, { useCodingAgent: false });

    const body = lastBody();
    expect(body.use_coding_agent).toBeUndefined();
    expect(body.repo_id).toBeUndefined();
    expect(body.folder).toBeUndefined();
  });

  it('retrying a draft after rollback with Lucidos scope clears prior repoId binding', async () => {
    // Simulate rollback state: a CC draft already has meta.repoId from a
    // previous sendCompose attempt (or some other source). User now picks
    // the default scope (Lucidos) and resends. The stale binding must NOT
    // leak into the chat body.
    const draftId = 'rollback-draft';
    focusedThreadId.value = draftId;
    putThread(draftId, { state: 'composing', channel: 'claude_code', repoId: 'stale-repo-uuid' });
    setDraft(draftId, { text: 'retry', image_hashes: [], mode: 'claude_code' });
    selectedScope.value = { kind: 'lucidos' };

    await sendCompose(draftId, { useCodingAgent: true });

    const body = lastBody();
    expect(body.use_coding_agent).toBe(true);
    expect(body.repo_id).toBeUndefined();
    expect(body.folder).toBeUndefined();
  });
});

/** The drawer row's context chip must not blink across the promotion. A draft
 *  row names its destination from the draft's own scope
 *  (`composeDraftContextName`); the started row names it from the bound meta
 *  (`threadContextName`). Both are on screen in the same place, one frame
 *  apart, so if the promotion drops a name it already resolved, the chip
 *  vanishes until the engine answers with `cc_repo_name` and then reappears.
 *  Each case asserts the two functions agree, not a literal, so the chip is
 *  pinned to continuity rather than to today's copy. */
describe('promotion keeps the destination chip the draft was already showing', () => {
  const REPOS = [
    { id: 'external-repo-uuid', name: 'example-repo', path: '/tmp/example-repo' },
    { id: 'lucidos-repo-uuid', name: LUCIDOS_SOURCE_REPO_NAME, path: '/tmp/lucidos' },
  ];

  beforeEach(() => {
    repositories.value = { status: 'loaded', data: REPOS };
  });

  function chipAfterSend(threadId: string): string | undefined {
    return threadContextName(threadMap.value.get(threadId)!.meta);
  }

  it('a Lucidos-source coding draft keeps its "Lucidos" chip', async () => {
    const draftId = 'lucidos-source-draft';
    focusedThreadId.value = draftId;
    putThread(draftId, { state: 'composing', channel: 'claude_code' });
    setDraft(draftId, { text: 'fix it', image_hashes: [], mode: 'claude_code' });
    patchComposeSelection(draftId, { scope: { kind: 'lucidos' } });

    const draftChip = composeDraftContextName('claude_code', { kind: 'lucidos' }, REPOS);
    await sendCompose(draftId, { useCodingAgent: true });

    expect(draftChip).toBe(LUCIDOS_SOURCE_REPO_NAME);
    expect(chipAfterSend(draftId)).toBe(draftChip);
  });

  it('an external-repo coding draft keeps the registry name', async () => {
    const draftId = 'external-source-draft';
    focusedThreadId.value = draftId;
    putThread(draftId, { state: 'composing', channel: 'claude_code' });
    setDraft(draftId, { text: 'fix it', image_hashes: [], mode: 'claude_code' });
    const scope: Scope = { kind: 'external', repoId: 'external-repo-uuid' };
    patchComposeSelection(draftId, { scope });

    const draftChip = composeDraftContextName('claude_code', scope, REPOS);
    await sendCompose(draftId, { useCodingAgent: true });

    expect(draftChip).toBe('example-repo');
    expect(chipAfterSend(draftId)).toBe(draftChip);
  });

  it('an app coding draft keeps the app id, and never files it as a repo name', async () => {
    const draftId = 'app-source-draft';
    focusedThreadId.value = draftId;
    putThread(draftId, { state: 'composing', channel: 'claude_code' });
    setDraft(draftId, { text: 'fix it', image_hashes: [], mode: 'claude_code' });
    const scope: Scope = { kind: 'app', appId: 'habit-tracker' };
    patchComposeSelection(draftId, { scope });

    const draftChip = composeDraftContextName('claude_code', scope, REPOS);
    await sendCompose(draftId, { useCodingAgent: true });

    expect(draftChip).toBe('habit-tracker');
    expect(chipAfterSend(draftId)).toBe(draftChip);
    // The app id names a folder, not a repository. Writing it to repoName would
    // make the chip right by accident and the Info row's "Repository" wrong on
    // purpose.
    expect(threadMap.value.get(draftId)!.meta.repoName).toBeUndefined();
  });

  it('a chat draft binds no repo name, so the started row stays chipless', async () => {
    const draftId = 'chat-source-draft';
    focusedThreadId.value = draftId;
    putThread(draftId, { state: 'composing', channel: 'chat' });
    setDraft(draftId, { text: 'hi', image_hashes: [], mode: null });
    // A repo scope left over from an earlier coding pick must not follow a chat
    // send onto the promoted thread.
    patchComposeSelection(draftId, { scope: { kind: 'external', repoId: 'external-repo-uuid' } });

    await sendCompose(draftId, { useCodingAgent: false });

    expect(chipAfterSend(draftId)).toBeUndefined();
    expect(threadMap.value.get(draftId)!.meta.repoName).toBeUndefined();
  });
});

/** A raw new send mints its own thread id client-side, so the engine has never
 *  seen it. The body must SAY that: an unknown `thread_id` with no create
 *  signal is a 404, not a thread conjured out of whatever the caller sent.
 *  That refusal is what stops a wrong-target write from looking exactly like a
 *  correct one (a message posted at the wrong engine used to materialize the
 *  thread there, so reading it back confirmed the mistake). See ADR 0050. */
describe('new_thread declares a client-minted thread id', () => {
  it('raw new send sets new_thread', async () => {
    await sendMessage('start something new');

    const body = lastBody();
    expect(body.new_thread).toBe(true);
    expect(body.thread_id).toBeTruthy();
  });

  it('follow-up on an existing thread does NOT set new_thread', async () => {
    const tid = 'existing-thread';
    focusedThreadId.value = tid;
    putThread(tid, { state: 'active', channel: 'chat', messageCount: 1 });

    await sendMessage('follow up');

    const body = lastBody();
    expect(body.new_thread).toBeUndefined();
    expect(body.thread_id).toBe(tid);
  });

  it('a coding-agent raw new send declares it too', async () => {
    selectedScope.value = { kind: 'lucidos' };

    await sendMessage('build the thing', undefined, { useCodingAgent: true });

    expect(lastBody().new_thread).toBe(true);
  });
});
