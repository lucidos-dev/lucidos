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
  if (typeof globalThis.document === 'undefined') (globalThis as any).document = {};
  if (!(globalThis.document as any).querySelector) (globalThis.document as any).querySelector = () => null;
  if (!(globalThis.document as any).querySelectorAll) (globalThis.document as any).querySelectorAll = () => [];
  if (typeof globalThis.requestAnimationFrame === 'undefined') {
    (globalThis as any).requestAnimationFrame = (cb: any) => { cb(); return 0; };
  }
});

import { makeThreadState } from './threads-test-helpers';
import { type ThreadState } from '../thread-events';
import { fetchArchivedCount } from '../../api/threads';
import { archiveThreadCount, threadMap } from '../store';
import { handleThreadEvent } from './thread-sync';
import { refreshArchivedCount } from './thread-loading';
import { _resetRowIntentsForTesting } from '../rowIntents';

vi.mock('../../api/threads', () => ({
  fetchThreads: vi.fn(),
  fetchThreadEvents: vi.fn().mockResolvedValue({ events: [], currentAggregate: null }),
  fetchArchivedCount: vi.fn(),
}));

const fetchCount = fetchArchivedCount as ReturnType<typeof vi.fn>;

function seedRow(id: string, section: 'inbox' | 'archived'): void {
  const map = new Map<string, ThreadState>(threadMap.value);
  map.set(id, makeThreadState(id, {
    meta: { id, title: id, channel: 'chat', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:01Z', status: 'idle', codingAgentChangeState: { kind: 'none' }, codingAgentIsExternalRepo: false, lastRevivedAt: '', messageCount: 1, section, activeChildrenCount: 0 },
  }));
  threadMap.value = map;
}

function frame(id: string, seq: number, type: string, section: 'inbox' | 'archived'): Record<string, unknown> {
  return {
    thread_id: id,
    seq,
    created: '2026-01-01T00:00:02Z',
    event: { type },
    aggregate: {
      threadId: id, title: id, channel: 'chat', initiator: 'user', createdAt: '',
      lastActivity: '2026-01-01T00:00:02Z', messageCount: 1, section, status: 'idle',
      activeChildrenCount: 0, totalChildrenCount: 0, blockingDescendantCount: 0, attentionDescendantCount: 0,
      codingAgentChangeState: { kind: 'none' }, codingAgentIsExternalRepo: false, isSaved: false,
      hasResponse: true, lastRevivedAt: null, parentThreadId: null, parentThreadTitle: null, state: 'active',
    },
  };
}

/** Lets the coalesced count refresh run to completion. */
async function settle(): Promise<void> {
  for (let i = 0; i < 5; i++) await Promise.resolve();
}

beforeEach(() => {
  threadMap.value = new Map();
  _resetRowIntentsForTesting();
  archiveThreadCount.value = 4;
  fetchCount.mockReset();
});

describe('Archive badge follows live section flips', () => {
  it('re-reads the count when a live frame moves a thread into the archive', async () => {
    seedRow('t', 'inbox');
    fetchCount.mockResolvedValue(5);
    handleThreadEvent(frame('t', 10, 'ThreadArchived', 'archived'));
    await settle();
    expect(fetchCount).toHaveBeenCalledTimes(1);
    expect(archiveThreadCount.value).toBe(5);
  });

  it('re-reads the count when a live frame moves a thread out of the archive', async () => {
    seedRow('t', 'archived');
    fetchCount.mockResolvedValue(3);
    handleThreadEvent(frame('t', 10, 'MessageReceived', 'inbox'));
    await settle();
    expect(archiveThreadCount.value).toBe(3);
  });

  it('leaves the count alone when the frame keeps the thread where it was', async () => {
    seedRow('t', 'inbox');
    handleThreadEvent(frame('t', 10, 'ThreadTitleGenerated', 'inbox'));
    await settle();
    expect(fetchCount).not.toHaveBeenCalled();
  });

  it('coalesces a burst of flips into a trailing re-read, so the last answer wins', async () => {
    seedRow('a', 'inbox');
    seedRow('b', 'inbox');
    seedRow('c', 'inbox');
    let release: (n: number) => void = () => {};
    fetchCount.mockImplementationOnce(() => new Promise<number>(r => { release = r; }));
    fetchCount.mockResolvedValueOnce(7);
    handleThreadEvent(frame('a', 10, 'ThreadArchived', 'archived'));
    handleThreadEvent(frame('b', 11, 'ThreadArchived', 'archived'));
    handleThreadEvent(frame('c', 12, 'ThreadArchived', 'archived'));
    release(5);
    await settle();
    expect(fetchCount).toHaveBeenCalledTimes(2);
    expect(archiveThreadCount.value).toBe(7);
  });

  it('re-reads the count for a thread the drawer never loaded, since its old section is unknown', async () => {
    fetchCount.mockResolvedValue(3);
    handleThreadEvent(frame('unloaded', 10, 'MessageReceived', 'inbox'));
    await settle();
    expect(fetchCount).toHaveBeenCalledTimes(1);
    expect(archiveThreadCount.value).toBe(3);
  });

  it('never lands an older flip read over a newer read started by another writer', async () => {
    seedRow('t', 'inbox');
    let release: (n: number) => void = () => {};
    fetchCount.mockImplementationOnce(() => new Promise<number>(r => { release = r; }));
    fetchCount.mockResolvedValueOnce(9);
    handleThreadEvent(frame('t', 10, 'ThreadArchived', 'archived'));
    await refreshArchivedCount();
    expect(archiveThreadCount.value).toBe(9);
    release(5);
    await settle();
    expect(archiveThreadCount.value).toBe(9);
  });
});
