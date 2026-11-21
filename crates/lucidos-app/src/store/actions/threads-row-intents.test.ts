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
import type { ThreadAggregate, ThreadState } from '../thread-events';
import { archiveThread, saveThread, unsaveThread } from '../../api/threads';
import { confirmState, focusedThreadId, threadMap, toasts } from '../store';
import { upsertThread } from './thread-loading';
import { handleThreadEvent } from './thread-sync';
import { handleArchiveThread, handleSaveThread, handleUnsaveThread } from './threads';
import { _resetRowIntentsForTesting, rowIntents } from '../rowIntents';

vi.mock('../../api/threads', () => ({
  fetchThreads: vi.fn(),
  fetchThreadEvents: vi.fn().mockResolvedValue({ events: [], currentAggregate: null }),
  fetchThreadMessages: vi.fn(),
  saveThread: vi.fn(),
  unsaveThread: vi.fn(),
  archiveThread: vi.fn(),
}));
vi.mock('../../components/chat/promptFocus', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../components/chat/promptFocus')>()),
  focusPromptNow: vi.fn(),
}));

type Mock = ReturnType<typeof vi.fn>;

/** A promise plus its resolver, so a test can hold a POST pending. */
function deferred<T>(): { promise: Promise<T>; resolve: (v: T) => void } {
  let resolve!: (v: T) => void;
  const promise = new Promise<T>((r) => { resolve = r; });
  return { promise, resolve };
}

/** One idle chat thread in Current, at summary version 3. */
function seed(overrides: Partial<ThreadState['meta']> = {}): void {
  const map = new Map<string, ThreadState>();
  map.set('t', makeThreadState('t', {
    meta: {
      id: 't', title: 'Thread', channel: 'chat', saved: false, createdAt: '',
      updatedAt: '2026-01-01T00:00:01Z', status: 'idle', codingAgentChangeState: { kind: 'none' },
      codingAgentIsExternalRepo: false, lastRevivedAt: '', messageCount: 1, section: 'inbox',
      activeChildrenCount: 0, summaryVersion: 3, ...overrides,
    },
  }));
  threadMap.value = map;
}

const meta = () => threadMap.value.get('t')!.meta;

/** An SSE frame for `t` whose aggregate says the row is at `version`. */
function frame(version: number, fields: Partial<ThreadAggregate>): void {
  handleThreadEvent({
    thread_id: 't',
    seq: 100 + version,
    created: '2026-01-01T00:00:02Z',
    event: { type: 'CodingAgentIdled' },
    aggregate: {
      threadId: 't', title: 'Thread', channel: 'chat', initiator: 'user', createdAt: '',
      lastActivity: '2026-01-01T00:00:02Z', messageCount: 1, section: 'inbox', status: 'idle',
      activeChildrenCount: 0, totalChildrenCount: 0, blockingDescendantCount: 0,
      attentionDescendantCount: 0, codingAgentChangeState: { kind: 'none' },
      codingAgentIsExternalRepo: false, isSaved: false, hasResponse: true, lastRevivedAt: null,
      parentThreadId: null, parentThreadTitle: null, state: 'active',
      ...fields, summaryVersion: version,
    },
  });
}

/** A list GET's summary for `t` at `version`. */
function listed(version: number, fields: { section: string; saved?: boolean }): void {
  upsertThread(threadMap.value, {
    thread_id: 't', title: 'Thread', channel: 'chat', last_activity: '2026-01-01T00:00:01Z',
    created_at: '', message_count: 1, status: 'idle', summary_version: version,
    coding_agent_change_state: { kind: 'none' }, section: fields.section,
  } as any, fields.saved ?? false);
}

beforeEach(() => {
  _resetRowIntentsForTesting();
  focusedThreadId.value = null;
  (archiveThread as Mock).mockReset();
  (saveThread as Mock).mockReset();
  (unsaveThread as Mock).mockReset();
});

// An optimistic row change is a pending intent until the row reaches the
// version the write returned (ADR 0437). A frame or a GET below that version
// was produced before the write, however late it arrives.
describe('an archive holds until the row reaches the version the server returned', () => {
  it('a pre-archive frame landing after the POST returned keeps the row archived', async () => {
    seed();
    (archiveThread as Mock).mockResolvedValueOnce({ archived: ['t'], summary_versions: { t: 5 } });

    await handleArchiveThread('t');
    frame(4, { section: 'inbox' });

    expect(meta().section).toBe('archived');
  });

  it('a pre-archive GET landing after the POST returned keeps the row archived', async () => {
    seed();
    (archiveThread as Mock).mockResolvedValueOnce({ archived: ['t'], summary_versions: { t: 5 } });

    await handleArchiveThread('t');
    listed(4, { section: 'inbox' });

    expect(meta().section).toBe('archived');
  });

  it('the row follows the server again once it reaches the returned version', async () => {
    seed();
    (archiveThread as Mock).mockResolvedValueOnce({ archived: ['t'], summary_versions: { t: 5 } });

    await handleArchiveThread('t');
    frame(5, { section: 'archived' });
    // Another device moved it back to Current.
    frame(6, { section: 'inbox' });

    expect(meta().section).toBe('inbox');
  });

  it('a frame that beat the response settles the archive as the response lands', async () => {
    seed();
    const post = deferred<{ archived: string[]; summary_versions: Record<string, number> }>();
    (archiveThread as Mock).mockReturnValueOnce(post.promise);

    const archiving = handleArchiveThread('t');
    frame(5, { section: 'archived' });
    post.resolve({ archived: ['t'], summary_versions: { t: 5 } });
    await archiving;
    frame(6, { section: 'inbox' });

    expect(meta().section).toBe('inbox');
  });

  it('a skipped member shows the latest server state, not the pre-click snapshot', async () => {
    seed({ status: 'waiting' });
    const post = deferred<{ archived: string[]; skipped: unknown[] }>();
    (archiveThread as Mock).mockReturnValueOnce(post.promise);

    const archiving = handleArchiveThread('t');
    // While the POST is out, the server moves the row on to running.
    frame(4, { section: 'inbox', status: 'running' });
    expect(meta().section).toBe('archived');
    post.resolve({ archived: [], skipped: [{ thread_id: 't', reason: 'not_archivable', message: 'left open' }] });
    await archiving;

    expect(meta()).toMatchObject({ section: 'inbox', status: 'running' });
  });
});

describe('a pin or unpin holds until the row reaches the version the server returned', () => {
  it('a pre-pin GET landing after the POST returned keeps the row pinned', async () => {
    seed();
    (saveThread as Mock).mockResolvedValueOnce({ summary_version: 5 });

    await handleSaveThread('t');
    listed(4, { section: 'inbox', saved: false });

    expect(meta().saved).toBe(true);
  });

  it('a pre-unpin GET landing after the POST returned keeps the row unpinned', async () => {
    seed({ saved: true });
    (unsaveThread as Mock).mockResolvedValueOnce({ summary_version: 5 });

    const unpinning = handleUnsaveThread('t');
    confirmState.value.resolve?.(true);
    await unpinning;
    listed(4, { section: 'inbox', saved: true });

    expect(meta().saved).toBe(false);
  });

  // Two writes on one row both in flight can commit in either order, so the
  // second waits until the first has answered.
  it('an unpin sends nothing while the pin it would undo is still in flight', async () => {
    seed();
    const pin = deferred<{ summary_version: number }>();
    (saveThread as Mock).mockReturnValueOnce(pin.promise);
    (unsaveThread as Mock).mockResolvedValueOnce({ summary_version: 6 });

    const pinning = handleSaveThread('t');
    const unpinning = handleUnsaveThread('t');
    confirmState.value.resolve?.(true);
    await unpinning;

    expect(unsaveThread).not.toHaveBeenCalled();
    expect(meta().saved).toBe(true);
    expect(toasts.value.map((t) => t.message)).toContainEqual(expect.stringMatching(/still pinning/i));
    pin.resolve({ summary_version: 5 });
    await pinning;
    frame(5, { isSaved: true });
    expect(meta().saved).toBe(true);
  });

  it('an archive sends nothing while a pin on the row is still in flight', async () => {
    seed();
    const pin = deferred<{ summary_version: number }>();
    (saveThread as Mock).mockReturnValueOnce(pin.promise);

    const pinning = handleSaveThread('t');
    const archiving = handleArchiveThread('t');
    confirmState.value.resolve?.(true);
    await archiving;

    expect(archiveThread).not.toHaveBeenCalled();
    expect(meta()).toMatchObject({ saved: true, section: 'inbox' });
    expect(toasts.value.map((t) => t.message)).toContainEqual(expect.stringMatching(/still pinning/i));
    pin.resolve({ summary_version: 5 });
    await pinning;
  });

  // An older engine acknowledges with no version. The client then waits for
  // any row newer than the one it held as it sent.
  it('an unversioned pin ends when its frame beat the response', async () => {
    seed();
    const pin = deferred<Record<string, never>>();
    (saveThread as Mock).mockReturnValueOnce(pin.promise);

    const pinning = handleSaveThread('t');
    frame(4, { isSaved: true });
    pin.resolve({});
    await pinning;

    expect(rowIntents.value.has('t')).toBe(false);
  });

  it('an unversioned archive ends when its frame beat the response', async () => {
    seed();
    const post = deferred<{ archived: string[] }>();
    (archiveThread as Mock).mockReturnValueOnce(post.promise);

    const archiving = handleArchiveThread('t');
    frame(4, { section: 'archived' });
    post.resolve({ archived: ['t'] });
    await archiving;

    expect(rowIntents.value.has('t')).toBe(false);
  });

  it('a refused pin shows the row as the server has it', async () => {
    seed();
    (saveThread as Mock).mockRejectedValueOnce(new Error('request timed out'));

    await handleSaveThread('t');

    expect(meta().saved).toBe(false);
  });
});
