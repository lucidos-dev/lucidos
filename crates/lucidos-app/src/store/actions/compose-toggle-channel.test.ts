
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
  cancelChat: vi.fn().mockResolvedValue({ canceled: true, taken_back_messages: [], failed_take_back_ids: [] }),
  stopClaudeCode: vi.fn().mockResolvedValue(undefined),
  putComposeOnThread: vi.fn().mockResolvedValue({ status: 'applied' }),
  ensureThreadStarted: vi.fn().mockResolvedValue(undefined),
  deleteThread: vi.fn().mockResolvedValue(undefined),
}));

vi.mock('./thread-navigation', () => ({
  pushThreadNavState: vi.fn(),
}));


vi.mock('./thread-loading', () => ({
  refreshThreadEvents: vi.fn().mockResolvedValue(true),
}));

vi.mock('./devices', () => ({
  getDeviceId: () => 'device-test',
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
} from '../store';
import { sendCompose } from './compose';
import { cancelCurrentExchange } from './chat';
import { setDraft, _resetComposeDraftsForTesting } from '../composeDrafts';
import { submitChat, cancelChat, stopClaudeCode } from '../../api/client';
import type { ChatRequestBody } from '../../api/types';
import type { ThreadMeta } from '../thread-events';

const mockedSubmitChat = vi.mocked(submitChat);
const mockedCancelChat = vi.mocked(cancelChat);
const mockedStopClaudeCode = vi.mocked(stopClaudeCode);

beforeEach(() => {
  threadMap.value = new Map();
  focusedThreadId.value = null;
  selectedScope.value = { kind: 'lucidos' };
  selectedCodingAgent.value = 'claude-code';
  connectionStatus.value = 'connected';
  mockedSubmitChat.mockClear();
  mockedCancelChat.mockClear();
  mockedStopClaudeCode.mockClear();
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
    channel: 'chat',
    initiator: 'user',
    saved: false,
    createdAt: '',
    updatedAt: '',
    status: 'idle',
    summaryVersion: 0,
    messageCount: 0,
    section: 'archived',
    activeChildrenCount: 0,
    totalChildrenCount: 0,
    blockingDescendantCount: 0,
    attentionDescendantCount: 0,
    archivableSubThreadCount: 0,
    codingAgentChangeState: { kind: 'none' },
    codingAgentIsExternalRepo: false,
    lastRevivedAt: '',
    state: 'composing',
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

describe('toggle-after-typing routes to the toggled channel (regression)', () => {
  it('Lucidos draft toggled to Claude before send routes to Claude Code', async () => {
    const draftId = 'toggle-lucidos-to-cc';
    focusedThreadId.value = draftId;
    putThread(draftId, { state: 'composing', channel: 'chat' });
    setDraft(draftId, { text: 'fix the bug', image_hashes: [], mode: 'claude_code' });

    await sendCompose(draftId, { useCodingAgent: true });

    const body = lastBody();
    expect(body.use_coding_agent).toBe(true);
  });

  it('Claude draft toggled to Lucidos before send routes to Lucidos', async () => {
    const draftId = 'toggle-cc-to-lucidos';
    focusedThreadId.value = draftId;
    putThread(draftId, { state: 'composing', channel: 'claude_code' });
    setDraft(draftId, { text: 'just a question', image_hashes: [], mode: 'lucidos' });

    await sendCompose(draftId, { useCodingAgent: false });

    const body = lastBody();
    expect(body.use_coding_agent).toBeUndefined();
  });

  it('locks thread.meta.channel to the picked mode at promotion (CC pick)', async () => {
    const draftId = 'lock-channel-cc';
    focusedThreadId.value = draftId;
    putThread(draftId, { state: 'composing', channel: 'chat' });
    setDraft(draftId, { text: 'fix the bug', image_hashes: [], mode: 'claude_code' });

    await sendCompose(draftId, { useCodingAgent: true });

    expect(threadMap.value.get(draftId)?.meta.channel).toBe('claude_code');
    expect(threadMap.value.get(draftId)?.meta.state).toBe('active');
  });

  it('locks thread.meta.channel to the picked mode at promotion (Lucidos pick)', async () => {
    const draftId = 'lock-channel-lucidos';
    focusedThreadId.value = draftId;
    putThread(draftId, { state: 'composing', channel: 'claude_code' });
    setDraft(draftId, { text: 'just a question', image_hashes: [], mode: 'lucidos' });

    await sendCompose(draftId, { useCodingAgent: false });

    expect(threadMap.value.get(draftId)?.meta.channel).toBe('chat');
    expect(threadMap.value.get(draftId)?.meta.state).toBe('active');
  });

  it('cancelCurrentExchange routes to stopClaudeCode after toggle-to-CC send', async () => {
    const draftId = 'cancel-after-toggle-cc';
    focusedThreadId.value = draftId;
    putThread(draftId, { state: 'composing', channel: 'chat' });
    setDraft(draftId, { text: 'fix the bug', image_hashes: [], mode: 'claude_code' });

    await sendCompose(draftId, { useCodingAgent: true });
    await cancelCurrentExchange(draftId);

    expect(mockedStopClaudeCode).toHaveBeenCalledTimes(1);
    expect(mockedCancelChat).not.toHaveBeenCalled();
  });

  it('cancelCurrentExchange routes to cancelChat after toggle-to-Lucidos send', async () => {
    const draftId = 'cancel-after-toggle-lucidos';
    focusedThreadId.value = draftId;
    putThread(draftId, { state: 'composing', channel: 'claude_code' });
    setDraft(draftId, { text: 'just a question', image_hashes: [], mode: 'lucidos' });

    await sendCompose(draftId, { useCodingAgent: false });
    await cancelCurrentExchange(draftId);

    expect(mockedCancelChat).toHaveBeenCalledTimes(1);
    expect(mockedStopClaudeCode).not.toHaveBeenCalled();
  });

  it('promoting a draft with matching channel + mode still works (no regression)', async () => {
    const draftId = 'happy-path';
    focusedThreadId.value = draftId;
    putThread(draftId, { state: 'composing', channel: 'claude_code' });
    setDraft(draftId, { text: 'fix it', image_hashes: [], mode: 'claude_code' });

    await sendCompose(draftId, { useCodingAgent: true });

    expect(lastBody().use_coding_agent).toBe(true);
    expect(threadMap.value.get(draftId)?.meta.channel).toBe('claude_code');
  });

  it('promoting a draft with null mode and CC option still routes via CC', async () => {
    const draftId = 'null-mode-cc';
    focusedThreadId.value = draftId;
    putThread(draftId, { state: 'composing', channel: 'chat' });
    setDraft(draftId, { text: 'fix it', image_hashes: [], mode: null });

    await sendCompose(draftId, { useCodingAgent: true });

    expect(lastBody().use_coding_agent).toBe(true);
    expect(threadMap.value.get(draftId)?.meta.channel).toBe('claude_code');
  });
});


describe('coding-agent backend binding at promotion', () => {
  it('codex pick binds onto meta and rides the request body', async () => {
    const draftId = 'codex-draft';
    focusedThreadId.value = draftId;
    selectedCodingAgent.value = 'codex';
    putThread(draftId, { state: 'composing', channel: 'claude_code' });
    setDraft(draftId, { text: 'port the parser', image_hashes: [], mode: 'claude_code' });

    await sendCompose(draftId, { useCodingAgent: true });

    const body = lastBody();
    expect(body.use_coding_agent).toBe(true);
    expect(body.coding_agent).toBe('codex');
    expect(threadMap.value.get(draftId)?.meta.codingAgent).toBe('codex');
  });

  it('claude-code default omits coding_agent from the body', async () => {
    const draftId = 'cc-default-draft';
    focusedThreadId.value = draftId;
    putThread(draftId, { state: 'composing', channel: 'claude_code' });
    setDraft(draftId, { text: 'fix it', image_hashes: [], mode: 'claude_code' });

    await sendCompose(draftId, { useCodingAgent: true });

    const body = lastBody();
    expect(body.use_coding_agent).toBe(true);
    expect(body.coding_agent).toBeUndefined();
  });

  it('lucidos sends never carry coding_agent even with codex picked', async () => {
    const draftId = 'lucidos-draft';
    focusedThreadId.value = draftId;
    selectedCodingAgent.value = 'codex';
    putThread(draftId, { state: 'composing', channel: 'chat' });
    setDraft(draftId, { text: 'just a question', image_hashes: [], mode: 'lucidos' });

    await sendCompose(draftId, { useCodingAgent: false });

    const body = lastBody();
    expect(body.use_coding_agent).toBeUndefined();
    expect(body.coding_agent).toBeUndefined();
  });
});
