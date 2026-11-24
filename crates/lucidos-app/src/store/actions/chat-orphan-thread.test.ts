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
  submitChat: vi.fn(),
  cancelChat: vi.fn(),
  stopClaudeCode: vi.fn(),
  putComposeOnThread: vi.fn().mockResolvedValue({ status: 'applied' }),
  ensureThreadStarted: vi.fn().mockResolvedValue(undefined),
  deleteThread: vi.fn().mockResolvedValue(undefined),
}));

vi.mock('./thread-navigation', () => ({
  pushThreadNavState: vi.fn(),
  removeThreadNavEntries: vi.fn(),
}));


vi.mock('./thread-loading', () => ({
  refreshThreadEvents: vi.fn().mockResolvedValue(true),
  forgetThreadEventsFailures: vi.fn(),
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
  connectionStatus,
} from '../store';
import { sendMessage } from './chat';
import { _resetComposeDraftsForTesting } from '../composeDrafts';
import { submitChat } from '../../api/client';
import { removeThreadNavEntries } from './thread-navigation';
import type { ThreadState } from '../thread-events';

const mockedSubmitChat = vi.mocked(submitChat);
const mockedRemoveThreadNavEntries = vi.mocked(removeThreadNavEntries);

beforeEach(() => {
  threadMap.value = new Map();
  focusedThreadId.value = null;
  selectedScope.value = { kind: 'lucidos' };
  connectionStatus.value = 'connected';
  mockedSubmitChat.mockReset();
  mockedRemoveThreadNavEntries.mockReset();
  _resetComposeDraftsForTesting();
});

function makeActiveThread(id: string, overrides: Partial<ThreadState['meta']> = {}): ThreadState {
  return {
    meta: {
      id,
      title: 'Existing Thread',
      channel: 'chat',
      initiator: 'user',
      saved: false,
      createdAt: '2026-05-11T19:00:00Z',
      updatedAt: '2026-05-11T19:00:00Z',
      status: 'idle',
      summaryVersion: 0,
      messageCount: 1,
      section: 'archived',
      activeChildrenCount: 0,
      totalChildrenCount: 0,
      blockingDescendantCount: 0, attentionDescendantCount: 0,
      codingAgentChangeState: { kind: 'none' },
      codingAgentIsExternalRepo: false,
      lastRevivedAt: '',
      state: 'active',
      latestTodoList: null,
      liveEventWaitCount: 0,
      liveEventWaits: [],
      ...overrides,
    },
    events: new Map(),
    streamingBuffer: '',
    eventsLoaded: true,
    eventsLoadFailed: false,
    lastDbSeq: 0,
    pendingUserMessages: [],
  };
}

describe('sendMessage rolls back orphan thread on send failure', () => {
  it('raw new send (focusedThreadId=null) removes the optimistic thread when submitChat fails', async () => {
    mockedSubmitChat.mockRejectedValueOnce(new Error('Failed to fetch'));
    expect(threadMap.value.size).toBe(0);

    await sendMessage('bæsj. google søkks');

    expect(threadMap.value.size).toBe(0);
    expect(focusedThreadId.value).toBe(null);
    expect(mockedRemoveThreadNavEntries).toHaveBeenCalledWith(expect.any(String));
  });

  it('successful raw new send keeps the thread in the map', async () => {
    mockedSubmitChat.mockResolvedValueOnce({ event_id: 'srv-evt' });
    expect(threadMap.value.size).toBe(0);

    await sendMessage('hello');

    expect(threadMap.value.size).toBe(1);
    const tid = focusedThreadId.value!;
    expect(tid).not.toBe(null);
    expect(threadMap.value.has(tid)).toBe(true);
  });

  it('failed follow-up on an existing active thread keeps the thread row', async () => {
    const tid = 'existing-thread';
    threadMap.value = new Map([[tid, makeActiveThread(tid)]]);
    focusedThreadId.value = tid;
    mockedSubmitChat.mockRejectedValueOnce(new Error('Failed to fetch'));

    await sendMessage('follow up that fails');

    expect(threadMap.value.has(tid)).toBe(true);
    expect(threadMap.value.get(tid)!.pendingUserMessages).toHaveLength(0);
    expect(mockedRemoveThreadNavEntries).not.toHaveBeenCalled();
  });

  it('failed send on a draft thread (state=composing) keeps the thread row', async () => {
    const tid = 'draft-thread';
    threadMap.value = new Map([[tid, makeActiveThread(tid, { state: 'composing', title: '' })]]);
    focusedThreadId.value = tid;
    mockedSubmitChat.mockRejectedValueOnce(new Error('Failed to fetch'));

    await sendMessage('first send fails');

    expect(threadMap.value.has(tid)).toBe(true);
  });
});
