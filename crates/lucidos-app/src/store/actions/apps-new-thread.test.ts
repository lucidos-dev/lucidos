import { describe, it, expect, beforeEach, vi } from 'vitest';

// Polyfill localStorage / document / rAF before store.ts loads at module level.
// vi.hoisted runs before any imports resolve (mirrors threads-focus-loading.test.ts).
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
  // scrollToBottom() (reached via addPendingMessage) needs these.
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
});

import { connectionStatus, focusedPane, focusedThreadId, mobileView, pendingChatMessage, threadMap, threadsLoaded } from '../store';
import { makeThreadState } from './threads-test-helpers';
import { submitNewApp } from './apps';
import { sendMessage } from './chat';
import { submitChat } from '../../api/client';

// Real store + real sendMessage chain; only the network call is stubbed so we
// can read the wire body's thread_id.
vi.mock('../../api/client', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../api/client')>()),
  API_BASE: '',
  submitChat: vi.fn().mockResolvedValue(undefined),
  listAppsApi: vi.fn().mockResolvedValue([]),
}));

/** Replicate PromptInput's pendingChatMessage consumer: drain the queued
 *  message and hand it to sendMessage exactly as the component does. The
 *  visible chat context is null here (no app/file open), which is what
 *  currentChatContext() returns in this scenario. */
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
  // Desktop by default (test-setup seeds innerWidth=1024); mobile cases override.
  (globalThis as any).innerWidth = 1024;
  focusedPane.value = 'thread';
  mobileView.value = 'thread';
  localStorage.clear();
  (submitChat as any).mockClear();
});

describe('submitNewApp — app creation starts a NEW thread', () => {
  it('posts the create-app message to a fresh thread, not the thread the user has open', async () => {
    // User is viewing an existing thread when they open the new-app form.
    threadMap.value = new Map([['t-open', makeThreadState('t-open')]]);
    focusedThreadId.value = 't-open';

    submitNewApp('Widget Maker', 'builds widgets');

    // The create-app prompt is queued for PromptInput to send.
    expect(pendingChatMessage.value).toBe('Create a new app called "Widget Maker": builds widgets');

    await drainPendingChatMessage();

    expect(submitChat).toHaveBeenCalledTimes(1);
    const body = (submitChat as any).mock.calls[0][0];
    expect(body.message).toBe('Create a new app called "Widget Maker": builds widgets');
    // The bug: this used to resolve to 't-open' (focusedThreadId), so the
    // create-app message was appended as a follow-up to whatever thread was
    // open. It must spawn a brand-new thread instead.
    expect(body.thread_id).not.toBe('t-open');
    // The previously-open thread must be left untouched — no optimistic user
    // message planted in it.
    expect(threadMap.value.get('t-open')!.pendingUserMessages).toHaveLength(0);
    // The user lands on the new thread.
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

  // The new-app form lives in the content pane (createNewApp → revealContentPane
  // → focusedPane='content' on desktop / mobileView='content' on mobile). After
  // submit, the new thread must surface on the thread pane — otherwise the user
  // is stranded on the now-empty content pane.
  it('desktop: re-activates the Threads pane group (was on the content pane)', async () => {
    (globalThis as any).innerWidth = 1024;
    focusedPane.value = 'content'; // viewing the new-app form

    submitNewApp('Widget Maker', 'builds widgets');
    await drainPendingChatMessage();

    expect(focusedPane.value).toBe('thread');
  });

  it('mobile: swipes to the thread pane (was on the content pane)', async () => {
    (globalThis as any).innerWidth = 375;
    mobileView.value = 'content'; // viewing the new-app form

    submitNewApp('Widget Maker', 'builds widgets');
    await drainPendingChatMessage();

    expect(mobileView.value).toBe('thread');
  });
});
