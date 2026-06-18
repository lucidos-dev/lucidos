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
  putComposeOnThread: vi.fn().mockResolvedValue(undefined),
  ensureThreadStarted: vi.fn().mockResolvedValue(undefined),
  deleteThread: vi.fn().mockResolvedValue(undefined),
}));

vi.mock('./thread-navigation', () => ({
  pushThreadNavState: vi.fn(),
}));

vi.mock('../../components/chat/scrollState', () => ({
  scrollToBottom: vi.fn(),
}));

vi.mock('./thread-loading', () => ({
  refreshThreadEvents: vi.fn().mockResolvedValue(undefined),
}));

vi.mock('./devices', () => ({
  getDeviceId: () => 'device-test',
}));

vi.mock('../../utils/platform', () => ({
  isTauri: () => false,
}));

import {
  focusedThreadId,
  threadMap,
  selectedScope,
  selectedCodingAgent,
  connectionStatus,
} from '../store';
import { sendMessage } from './chat';
import { sendCompose } from './compose';
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
    selectedScope.value = { kind: 'app', appId: 'momentum' };

    await sendMessage('fix the chart bug', undefined, { useCodingAgent: true });

    const body = lastBody();
    expect(body.use_coding_agent).toBe(true);
    expect(body.folder).toBe('data/apps/momentum');
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
      codingAgentFolder: '/some/workspace/data/apps/momentum',
    });
    selectedScope.value = { kind: 'lucidos' };

    await sendMessage('keep going', undefined, { useCodingAgent: true });

    const body = lastBody();
    expect(body.use_coding_agent).toBe(true);
    expect(body.folder).toBe('data/apps/momentum');
    expect(body.repo_id).toBeUndefined();
  });
});

describe('sendCompose carries dropdown scope through to chat body (real flow)', () => {
  it('promoting a CC draft to active sends folder from external scope', async () => {
    const draftId = 'draft-thread';
    focusedThreadId.value = draftId;
    putThread(draftId, { state: 'composing', channel: 'claude_code' });
    setDraft(draftId, { text: 'fix it', image_hashes: [], mode: 'claude_code' });
    selectedScope.value = { kind: 'external', repoId: 'external-repo-uuid' };

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
    selectedScope.value = { kind: 'app', appId: 'momentum' };

    await sendCompose(draftId, { useCodingAgent: true });

    const body = lastBody();
    expect(body.use_coding_agent).toBe(true);
    expect(body.folder).toBe('data/apps/momentum');
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
