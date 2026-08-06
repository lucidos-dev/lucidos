/**
 * Verifies sendMessage distinguishes transport errors (in-thread failed
 * exchange — preserves the user's text) from HTTP / unknown errors (toast +
 * rollback, covered by chat-orphan-thread.test.ts). Stale `connectionStatus`
 * must NOT short-circuit a send that would have succeeded.
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
  // getUnreachableEngineMsg() reads window.location.origin when API_BASE is
  // empty (the dev/test default). Stub a stable origin so the message is
  // deterministic without leaking test machine state.
  if (typeof globalThis.window === 'undefined') {
    (globalThis as any).window = {};
  }
  if (typeof (globalThis as any).window.location === 'undefined') {
    (globalThis as any).window.location = { origin: 'https://localhost:5173' };
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
import { groupIntoExchanges, exchangeError, exchangeUserMessage } from '../thread-events';

const mockedSubmitChat = vi.mocked(submitChat);

beforeEach(() => {
  threadMap.value = new Map();
  focusedThreadId.value = null;
  selectedScope.value = { kind: 'lucidos' };
  connectionStatus.value = 'connected';
  mockedSubmitChat.mockReset();
  _resetComposeDraftsForTesting();
});

describe('sendMessage transport-error handling', () => {
  it('TypeError("Failed to fetch") → renders in-thread ResponseFailed exchange (preserves message)', async () => {
    mockedSubmitChat.mockRejectedValueOnce(new TypeError('Failed to fetch'));

    await sendMessage('important question');

    const tid = focusedThreadId.value;
    expect(tid).not.toBeNull();
    const thread = threadMap.value.get(tid!);
    expect(thread).toBeDefined();

    const exchanges = groupIntoExchanges(thread!.events);
    expect(exchanges).toHaveLength(1);
    expect(exchangeUserMessage(exchanges[0])).toBe('important question');
    expect(exchangeError(exchanges[0])?.message).toBeTruthy();
  });

  it('error message wording is honest about scope (no "engine disconnected" claim)', async () => {
    mockedSubmitChat.mockRejectedValueOnce(new TypeError('Failed to fetch'));

    await sendMessage('hi');

    const tid = focusedThreadId.value!;
    const exchanges = groupIntoExchanges(threadMap.value.get(tid)!.events);
    const errorText = exchangeError(exchanges[0])!.message;
    expect(errorText.toLowerCase()).not.toContain('disconnected');
    expect(errorText.toLowerCase()).toMatch(/could not reach|unable to reach|cannot reach/);
  });

  it('Safari "Load failed" TypeError is also treated as transport error', async () => {
    // isTransportError covers three TypeError messages (Chrome, Safari,
    // Firefox). Each browser variant gets a separate test so a future regex
    // narrowing in api/client.ts surfaces here per browser, not as one fail.
    mockedSubmitChat.mockRejectedValueOnce(new TypeError('Load failed'));

    await sendMessage('safari send');

    const tid = focusedThreadId.value!;
    const exchanges = groupIntoExchanges(threadMap.value.get(tid)!.events);
    expect(exchanges).toHaveLength(1);
    expect(exchangeError(exchanges[0])?.message).toBeTruthy();
  });

  it('stale isConnected=false + submitChat resolving → normal exchange, no false-positive failure', async () => {
    connectionStatus.value = 'disconnected';
    mockedSubmitChat.mockResolvedValueOnce({ event_id: 'srv-evt' });

    await sendMessage('this should go through');

    expect(mockedSubmitChat).toHaveBeenCalledTimes(1);
    const tid = focusedThreadId.value!;
    const thread = threadMap.value.get(tid)!;
    const exchanges = groupIntoExchanges(thread.events);
    for (const ex of exchanges) {
      expect(exchangeError(ex)).toBeNull();
    }
  });
});
