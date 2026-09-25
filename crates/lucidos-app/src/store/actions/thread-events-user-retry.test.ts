import { describe, it, expect, beforeEach, vi } from 'vitest';

// Polyfill localStorage before store.ts is imported at module level. Mirrors
// the scaffolding in thread-loading-stale.test.ts, which drives the same module.
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
});

import { makeThreadState } from './threads-test-helpers';
import { connectionStatus, threadMap, toasts } from '../store';
import {
  _resetThreadEventsFailuresForTesting,
  clearThreadFetchGuards,
  forceRetryThreadEvents,
  retryThreadEvents,
} from './thread-loading';
import { fetchThreadEvents } from '../../api/threads';

vi.mock('../../api/threads', () => ({
  fetchThreads: vi.fn(),
  fetchThreadById: vi.fn(),
  fetchThreadEvents: vi.fn(),
  fetchThreadMessages: vi.fn(),
  saveThread: vi.fn().mockResolvedValue(undefined),
  archiveThread: vi.fn().mockResolvedValue({ archived: [] }),
}));

vi.mock('../../components/chat/promptFocus', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../components/chat/promptFocus')>()),
  focusPromptNow: vi.fn(),
  composeHandlers: vi.fn(),
}));

vi.mock('../../api/client', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../api/client')>()),
  API_BASE: '',
}));

const fetchEvents = fetchThreadEvents as unknown as ReturnType<typeof vi.fn>;

async function settle(): Promise<void> {
  for (let i = 0; i < 10; i++) await Promise.resolve();
}

beforeEach(() => {
  threadMap.value = new Map([['t1', makeThreadState('t1')]]);
  connectionStatus.value = 'connected';
  toasts.value = [];
  _resetThreadEventsFailuresForTesting();
  clearThreadFetchGuards();
  fetchEvents.mockReset();
  fetchEvents.mockRejectedValue(new Error('engine refused'));
});

describe('the Retry button on a thread that failed to load', () => {
  it('fetches again after the watchdog spent its one forced retry', async () => {
    forceRetryThreadEvents('t1');
    await settle();
    forceRetryThreadEvents('t1');
    await settle();
    // The watchdog is capped at one retry, so it cannot loop.
    expect(fetchEvents).toHaveBeenCalledTimes(1);

    retryThreadEvents('t1');
    await settle();
    // A press is the user asking, and the cap is not theirs.
    expect(fetchEvents).toHaveBeenCalledTimes(2);

    retryThreadEvents('t1');
    await settle();
    expect(fetchEvents).toHaveBeenCalledTimes(3);
  });
});
