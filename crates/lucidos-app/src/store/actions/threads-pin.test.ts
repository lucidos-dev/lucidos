import { describe, it, expect, beforeEach, vi } from 'vitest';

// Polyfill localStorage/document before store.ts loads at module level
// (vi.hoisted runs before imports resolve). Mirrors threads-archive.test.ts.
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
import { ApiError } from '../../api/client';
import { saveThread } from '../../api/threads';
import { threadMap, toasts } from '../store';
import { handleSaveThread } from './threads';

// Only saveThread is exercised here; the other named exports resolve to
// undefined, which is fine because handleSaveThread never calls them.
vi.mock('../../api/threads', () => ({
  saveThread: vi.fn(),
}));

const mockSave = vi.mocked(saveThread);

describe('handleSaveThread — pin error handling', () => {
  beforeEach(() => {
    threadMap.value = new Map([['t1', makeThreadState('t1', { meta: { saved: false } })]]);
    toasts.value = [];
    mockSave.mockReset();
  });

  it('keeps the optimistic pin and stays quiet when the server 409s (already saved)', async () => {
    // A duplicate/racing save: the first request already flipped is_saved=TRUE,
    // so a stale second /threads/save 409s. The desired end-state (pinned) holds
    // — the icon must NOT revert and no error toast must fire.
    mockSave.mockRejectedValue(new ApiError(409, 'Thread cannot be saved in its current state'));

    await handleSaveThread('t1');

    expect(threadMap.value.get('t1')!.meta.saved).toBe(true);
    expect(toasts.value).toHaveLength(0);
  });

  it('reverts the optimistic pin and toasts on a genuine failure (500)', async () => {
    mockSave.mockRejectedValue(new ApiError(500, 'boom'));

    await handleSaveThread('t1');

    expect(threadMap.value.get('t1')!.meta.saved).toBe(false);
    expect(toasts.value).toHaveLength(1);
  });

  it('leaves the thread pinned on success', async () => {
    mockSave.mockResolvedValue(undefined as unknown as void);

    await handleSaveThread('t1');

    expect(mockSave).toHaveBeenCalledWith('t1');
    expect(threadMap.value.get('t1')!.meta.saved).toBe(true);
    expect(toasts.value).toHaveLength(0);
  });
});
