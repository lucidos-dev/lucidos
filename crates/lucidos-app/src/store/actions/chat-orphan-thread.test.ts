/**
 * Bug: when `sendMessage` runs with `focusedThreadId === null` (raw new send),
 * it inserts an optimistic thread into `threadMap` (title = first 40 chars of
 * the message, state='active', status='running'). If `submitChat` then throws
 * — engine restarting, browser offline, transport error — the catch block
 * only removes the pending message; the optimistic thread itself stays in
 * `threadMap` forever. The user sees a phantom row in the Active drawer
 * section that vanishes on refresh because the API has no record of it.
 *
 * Fix: when the failed send was for a thread `sendMessage` itself created
 * (`threadBeforeSend === undefined`), the catch must also drop the thread
 * from `threadMap` — same shape as `compose.ts`'s `rollbackOptimistic`.
 *
 * Established follow-ups (`threadBeforeSend.meta.state === 'active'`) keep
 * their thread row on failure: it has real persisted content the user must
 * not lose just because one send round-trip failed.
 */
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

vi.mock('../../components/chat/scrollState', () => ({
  scrollToBottom: vi.fn(),
}));

vi.mock('./thread-loading', () => ({
  refreshThreadEvents: vi.fn().mockResolvedValue(true),
  // The rollback under test removes the row, so it also clears that thread's
  // entry in the thread-events failure maps.
  forgetThreadEventsFailures: vi.fn(),
}));

vi.mock('./devices', () => ({
  getDeviceId: () => 'device-test',
  pendingDeviceRegistration: vi.fn(),
}));

vi.mock('../../utils/platform', () => ({
  isTauri: () => false,
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
      messageCount: 1,
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

    // Without the fix the optimistic thread stays — phantom row in Active.
    expect(threadMap.value.size).toBe(0);
    expect(focusedThreadId.value).toBe(null);
    // Nav cleanup pairs with the threadMap drop so Back/Forward can't later
    // restore an id whose threadMap entry no longer exists.
    expect(mockedRemoveThreadNavEntries).toHaveBeenCalledWith(expect.any(String));
  });

  it('successful raw new send keeps the thread in the map', async () => {
    mockedSubmitChat.mockResolvedValueOnce({ event_id: 'srv-evt' });
    expect(threadMap.value.size).toBe(0);

    await sendMessage('hello');

    // Real send: server has the thread, client must keep its optimistic row
    // for the SSE events to land on.
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

    // The thread predates the send and has real content — it must NOT be
    // removed just because one round-trip failed.
    expect(threadMap.value.has(tid)).toBe(true);
    expect(threadMap.value.get(tid)!.pendingUserMessages).toHaveLength(0);
    expect(mockedRemoveThreadNavEntries).not.toHaveBeenCalled();
  });

  it('failed send on a draft thread (state=composing) keeps the thread row', async () => {
    // This path is reached when sendCompose's pre-flip is still in flight or
    // a caller routes a composing thread directly through sendMessage. The
    // draft thread has a server-side row from POST /api/v1/threads — wiping
    // it here would diverge the client from a server row that still exists.
    const tid = 'draft-thread';
    threadMap.value = new Map([[tid, makeActiveThread(tid, { state: 'composing', title: '' })]]);
    focusedThreadId.value = tid;
    mockedSubmitChat.mockRejectedValueOnce(new Error('Failed to fetch'));

    await sendMessage('first send fails');

    expect(threadMap.value.has(tid)).toBe(true);
  });
});
