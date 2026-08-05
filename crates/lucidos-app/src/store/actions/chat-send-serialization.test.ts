/**
 * Two rapid sends on one thread must reach the engine in the order the user
 * pressed send. Before this suite, `sendMessage` fired each POST without
 * waiting for the previous one, so two in-flight requests could be delivered
 * out of order by the network and the engine recorded them reversed
 * (`docs/plans/2026-07-30-serialize-chat-sends-per-thread.md`): the later
 * message won the race, started the turn, and the earlier one was queued and
 * injected into it minutes later, so the model read them backwards too.
 *
 * The chain must not cost anything the user can see: the optimistic rows still
 * appear instantly, a failed send never blocks the next one, and a POST that
 * never settles releases the chain rather than swallowing every later message
 * (`mutatingFetch` has no client-side timeout).
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';

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
  putComposeOnThread: vi.fn().mockResolvedValue(undefined),
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
}));

vi.mock('../../utils/platform', () => ({
  isTauri: vi.fn(() => false),
}));

vi.mock('../../utils/tauri', () => ({
  getWebviewContent: vi.fn(),
}));

import { focusedThreadId, threadMap, selectedScope, connectionStatus, panelOverlay } from '../store';
import { sendMessage, SEND_CHAIN_MAX_WAIT_MS } from './chat';
import { makeOptimisticThreadState } from '../thread-events';
import { _resetComposeDraftsForTesting } from '../composeDrafts';
import { submitChat } from '../../api/client';
import { isTauri } from '../../utils/platform';
import { getWebviewContent } from '../../utils/tauri';

const mockedSubmitChat = vi.mocked(submitChat);
const mockedIsTauri = vi.mocked(isTauri);
const mockedGetWebviewContent = vi.mocked(getWebviewContent);

const THREAD_A = '11111111-1111-4111-8111-111111111111';
const THREAD_B = '22222222-2222-4222-8222-222222222222';

/** A promise plus its resolve/reject, so a test can hold a POST open. */
function deferred<T>(): { promise: Promise<T>; resolve: (v: T) => void; reject: (e: unknown) => void } {
  let resolve!: (v: T) => void;
  let reject!: (e: unknown) => void;
  const promise = new Promise<T>((res, rej) => { resolve = res; reject = rej; });
  return { promise, resolve, reject };
}

function seedActiveThread(id: string): void {
  const next = new Map(threadMap.value);
  next.set(id, makeOptimisticThreadState({
    id,
    title: 'seeded',
    channel: 'chat',
    initiator: 'user',
    eventsLoaded: true,
    state: 'active',
    status: 'idle',
  }));
  threadMap.value = next;
}

/** Let queued microtasks run so a chained send can reach `submitChat`. */
async function flush(): Promise<void> {
  for (let i = 0; i < 5; i++) await Promise.resolve();
}

const sentMessages = () => mockedSubmitChat.mock.calls.map(c => (c[0] as { message: string }).message);

beforeEach(() => {
  threadMap.value = new Map();
  focusedThreadId.value = null;
  selectedScope.value = { kind: 'lucidos' };
  connectionStatus.value = 'connected';
  panelOverlay.value = null;
  mockedSubmitChat.mockReset();
  mockedIsTauri.mockReset();
  mockedIsTauri.mockReturnValue(false);
  mockedGetWebviewContent.mockReset();
  _resetComposeDraftsForTesting();
});

afterEach(() => {
  vi.useRealTimers();
});

describe('per-thread send serialization', () => {
  it('holds the second POST until the first settles, and sends them in order', async () => {
    seedActiveThread(THREAD_A);
    const first = deferred<{ event_id: string }>();
    mockedSubmitChat
      .mockReturnValueOnce(first.promise)
      .mockResolvedValueOnce({ event_id: 'e2' });

    const p1 = sendMessage('first', undefined, { threadId: THREAD_A, focus: false });
    const p2 = sendMessage('second', undefined, { threadId: THREAD_A, focus: false });
    await flush();

    // Only the first is on the wire while it is still in flight.
    expect(sentMessages()).toEqual(['first']);

    first.resolve({ event_id: 'e1' });
    await Promise.all([p1, p2]);

    expect(sentMessages()).toEqual(['first', 'second']);
  });

  it('renders both optimistic rows immediately, in send order, while the first POST is open', async () => {
    seedActiveThread(THREAD_A);
    const first = deferred<{ event_id: string }>();
    mockedSubmitChat
      .mockReturnValueOnce(first.promise)
      .mockResolvedValueOnce({ event_id: 'e2' });

    const p1 = sendMessage('first', undefined, { threadId: THREAD_A, focus: false });
    const p2 = sendMessage('second', undefined, { threadId: THREAD_A, focus: false });
    await flush();

    const pending = threadMap.value.get(THREAD_A)!.pendingUserMessages;
    expect(pending.map(m => m.text)).toEqual(['first', 'second']);

    first.resolve({ event_id: 'e1' });
    await Promise.all([p1, p2]);
  });

  it('a failed send does not block the next one', async () => {
    seedActiveThread(THREAD_A);
    const first = deferred<{ event_id: string }>();
    mockedSubmitChat
      .mockReturnValueOnce(first.promise)
      .mockResolvedValueOnce({ event_id: 'e2' });

    const p1 = sendMessage('first', undefined, { threadId: THREAD_A, focus: false });
    const p2 = sendMessage('second', undefined, { threadId: THREAD_A, focus: false });
    await flush();

    first.reject(new TypeError('Failed to fetch'));
    await Promise.all([p1, p2]);

    expect(sentMessages()).toEqual(['first', 'second']);
  });

  it('releases the chain when a POST never settles, so later messages still go out', async () => {
    vi.useFakeTimers();
    seedActiveThread(THREAD_A);
    const never = deferred<{ event_id: string }>();
    mockedSubmitChat
      .mockReturnValueOnce(never.promise)
      .mockResolvedValueOnce({ event_id: 'e2' });

    void sendMessage('first', undefined, { threadId: THREAD_A, focus: false });
    const p2 = sendMessage('second', undefined, { threadId: THREAD_A, focus: false });
    await vi.advanceTimersByTimeAsync(SEND_CHAIN_MAX_WAIT_MS - 1);
    expect(sentMessages()).toEqual(['first']);

    await vi.advanceTimersByTimeAsync(2);
    await p2;

    expect(sentMessages()).toEqual(['first', 'second']);
  });

  it('does not serialize across different threads', async () => {
    seedActiveThread(THREAD_A);
    seedActiveThread(THREAD_B);
    const openOnA = deferred<{ event_id: string }>();
    mockedSubmitChat
      .mockReturnValueOnce(openOnA.promise)
      .mockResolvedValueOnce({ event_id: 'e2' });

    const p1 = sendMessage('on A', undefined, { threadId: THREAD_A, focus: false });
    const p2 = sendMessage('on B', undefined, { threadId: THREAD_B, focus: false });
    await flush();

    // B must not wait behind A's open POST.
    expect(sentMessages()).toEqual(['on A', 'on B']);

    openOnA.resolve({ event_id: 'e1' });
    await Promise.all([p1, p2]);
  });

  it('claims the chain slot at call time, so an await before the POST cannot reorder', async () => {
    // The Tauri panel path awaits `getWebviewContent()` while building the
    // body. Two of those resolve in whatever order the webview answers, so a
    // chain that took its slot at POST time would let the second send overtake
    // the first and reverse them, which is the very bug the chain prevents.
    seedActiveThread(THREAD_A);
    // `panelUrl` is computed from the overlay, so open a url-preview overlay.
    panelOverlay.value = { type: 'url-preview', url: 'https://example.com/page' };
    mockedIsTauri.mockReturnValue(true);
    const slowExtract = deferred<{ title: string; content: string }>();
    mockedGetWebviewContent
      .mockReturnValueOnce(slowExtract.promise)
      .mockResolvedValueOnce({ title: 'second', content: 'second body' });
    mockedSubmitChat.mockResolvedValue({ event_id: 'e' });

    const p1 = sendMessage('first', undefined, { threadId: THREAD_A, focus: false });
    const p2 = sendMessage('second', undefined, { threadId: THREAD_A, focus: false });

    // The SECOND send's extraction finishes first, so it reaches the POST
    // first. It must still wait, because the first send already holds the slot.
    await flush();
    expect(sentMessages()).toEqual([]);

    slowExtract.resolve({ title: 'first', content: 'first body' });
    await Promise.all([p1, p2]);

    expect(sentMessages()).toEqual(['first', 'second']);
  });

  it('dispatches a lone send synchronously, without deferring a microtask', async () => {
    // An empty chain must not cost even one microtask: `sendMessage` issues the
    // POST inside the caller's synchronous turn, and callers observe that (the
    // compose suite asserts on the fetch mock right after `sendFollowup`,
    // without awaiting). Awaiting an already-resolved `waitForTurn` broke three
    // of those tests, so the empty case skips the await entirely.
    seedActiveThread(THREAD_A);
    mockedSubmitChat.mockResolvedValue({ event_id: 'e' });

    const p = sendMessage('lone', undefined, { threadId: THREAD_A, focus: false });
    expect(sentMessages()).toEqual(['lone']);
    await p;
  });

  it('a settled chain is dropped, so a later lone send is not delayed', async () => {
    seedActiveThread(THREAD_A);
    mockedSubmitChat.mockResolvedValue({ event_id: 'e' });

    await sendMessage('first', undefined, { threadId: THREAD_A, focus: false });
    // No pending predecessor: this must reach the wire without awaiting a timer.
    const p = sendMessage('second', undefined, { threadId: THREAD_A, focus: false });
    await flush();
    expect(sentMessages()).toEqual(['first', 'second']);
    await p;
  });
});
