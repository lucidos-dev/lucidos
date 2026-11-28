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
});

import { connectionStatus, focusedPane, focusedThreadId, mobileView, pendingChatMessage, threadMap, threadsLoaded } from '../store';
import { makeThreadState } from './threads-test-helpers';
import { submitNewApp } from './apps';
import { sendMessage } from './chat';
import { submitChat } from '../../api/client';

vi.mock('../../api/client', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../api/client')>()),
  API_BASE: '',
  submitChat: vi.fn().mockResolvedValue({ event_id: 'e-1' }),
  listAppsApi: vi.fn().mockResolvedValue([]),
}));

async function drainPendingChatMessage(): Promise<void> {
  const msg = pendingChatMessage.value;
  if (!msg) return;
  pendingChatMessage.value = null;
  await sendMessage(msg, undefined, { context: null });
}

beforeEach(() => {
  threadMap.value = new Map();
  focusedThreadId.value = null;
  pendingChatMessage.value = null;
  threadsLoaded.value = false;
  connectionStatus.value = 'connected';
  (globalThis as any).innerWidth = 1024;
  focusedPane.value = 'thread';
  mobileView.value = 'thread';
  localStorage.clear();
  (submitChat as any).mockClear();
});

describe('submitNewApp — app creation starts a NEW thread', () => {
  it('posts the create-app message to a fresh thread, not the thread the user has open', async () => {
    threadMap.value = new Map([['t-open', makeThreadState('t-open')]]);
    focusedThreadId.value = 't-open';

    submitNewApp('Widget Maker', 'builds widgets');

    expect(pendingChatMessage.value).toBe('Create a new app called "Widget Maker": builds widgets');

    await drainPendingChatMessage();

    expect(submitChat).toHaveBeenCalledTimes(1);
    const body = (submitChat as any).mock.calls[0][0];
    expect(body.message).toBe('Create a new app called "Widget Maker": builds widgets');
    expect(body.thread_id).not.toBe('t-open');
    expect(threadMap.value.get('t-open')!.pendingUserMessages).toHaveLength(0);
    expect(focusedThreadId.value).toBe(body.thread_id);
  });

  it('creates a new thread when nothing is focused (compose view)', async () => {
    focusedThreadId.value = null;

    submitNewApp('Solo', 'no thread open');
    await drainPendingChatMessage();

    expect(submitChat).toHaveBeenCalledTimes(1);
    const body = (submitChat as any).mock.calls[0][0];
    expect(body.thread_id).toBeTruthy();
    expect(focusedThreadId.value).toBe(body.thread_id);
  });

  it('desktop: re-activates the Threads pane group (was on the content pane)', async () => {
    (globalThis as any).innerWidth = 1024;
    focusedPane.value = 'content';

    submitNewApp('Widget Maker', 'builds widgets');
    await drainPendingChatMessage();

    expect(focusedPane.value).toBe('thread');
  });

  it('mobile: swipes to the thread pane (was on the content pane)', async () => {
    (globalThis as any).innerWidth = 375;
    mobileView.value = 'content';

    submitNewApp('Widget Maker', 'builds widgets');
    await drainPendingChatMessage();

    expect(mobileView.value).toBe('thread');
  });
});
