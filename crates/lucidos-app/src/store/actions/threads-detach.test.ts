/** Moving a child thread to top level from the ⋯ menu (ADR 0278): the copy,
 *  who is offered the item, the confirm-then-call flow, and a list refresh
 *  that must not re-nest the moved row. */

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

vi.mock('../../api/threads', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../api/threads')>()),
  detachThread: vi.fn().mockResolvedValue(undefined),
}));

import { detachThread } from '../../api/threads';
import { confirmState, threadMap, toasts } from '../store';
import { makeThreadState } from './threads-test-helpers';
import { canMoveToTopLevel, detachConfirmation, handleDetachThread } from './threads-detach';
import { upsertThread } from './thread-loading';

function seedFamily(): void {
  threadMap.value = new Map([
    ['parent', makeThreadState('parent', { meta: { title: 'Plan the release' } })],
    ['child', makeThreadState('child', { meta: { title: 'Write the notes', parentThreadId: 'parent' } })],
  ]);
}

/** Answer the dialog the handler opened, once it is up. */
async function answerConfirm(ok: boolean): Promise<void> {
  await vi.waitFor(() => expect(confirmState.value.visible).toBe(true));
  confirmState.value.resolve?.(ok);
}

const closedConfirm = confirmState.peek();

beforeEach(() => {
  confirmState.value = closedConfirm;
  vi.mocked(detachThread).mockClear();
  toasts.value = [];
  seedFamily();
});

describe('detachConfirmation', () => {
  it('names both threads and says nothing is stopped and it cannot be undone', () => {
    const { title, message } = detachConfirmation('Write the notes', 'Plan the release');
    expect(title).toBe('Move to top level?');
    expect(message).toContain('Move "Write the notes" out of "Plan the release"?');
    expect(message).toContain('Nothing is stopped and no work is lost');
    expect(message).toContain('"Plan the release" stops waiting for it');
    expect(message).toContain('You cannot undo this.');
  });

  it('falls back to generic names for an untitled thread', () => {
    expect(detachConfirmation(' ', '').message).toContain('Move "this thread" out of "its parent thread"?');
  });
});

describe('canMoveToTopLevel', () => {
  it('offers the item only on a thread with a parent', () => {
    expect(canMoveToTopLevel('child')).toBe(true);
    expect(canMoveToTopLevel('parent')).toBe(false);
    expect(canMoveToTopLevel('missing')).toBe(false);
  });
});

describe('handleDetachThread', () => {
  it('moves the thread once the user confirms, with the default button', async () => {
    const done = handleDetachThread('child');
    await vi.waitFor(() => expect(confirmState.value.visible).toBe(true));
    expect(confirmState.value.okLabel).toBe('Move out');
    expect(confirmState.value.variant).toBe('default');
    confirmState.value.resolve?.(true);
    await done;
    expect(detachThread).toHaveBeenCalledWith('child');
  });

  it('does nothing when the user cancels', async () => {
    const done = handleDetachThread('child');
    await answerConfirm(false);
    await done;
    expect(detachThread).not.toHaveBeenCalled();
  });

  it('asks nothing for a thread that has no parent', async () => {
    await handleDetachThread('parent');
    expect(confirmState.value.visible).toBe(false);
    expect(detachThread).not.toHaveBeenCalled();
  });

  it('says so when the engine refuses', async () => {
    vi.mocked(detachThread).mockRejectedValueOnce(new Error('Thread child is already at top level.'));
    const done = handleDetachThread('child');
    await answerConfirm(true);
    await done;
    expect(toasts.value.some((t) => t.message.includes('Could not move the thread to top level'))).toBe(true);
  });
});

describe('a list refresh after the move', () => {
  const summary = (parent: string | null | undefined) => ({
    thread_id: 'child',
    title: 'Write the notes',
    channel: 'chat',
    last_activity: '2026-01-01T00:00:00Z',
    created_at: '',
    message_count: 1,
    section: 'inbox',
    status: 'idle',
    ...(parent === undefined ? {} : { parent_thread_id: parent, parent_thread_title: parent && 'Plan the release' }),
  }) as any;

  it('clears a parent the engine no longer reports', () => {
    upsertThread(threadMap.value, summary(null), false);
    expect(threadMap.value.get('child')?.meta.parentThreadId).toBeUndefined();
    expect(threadMap.value.get('child')?.meta.parentThreadTitle).toBeUndefined();
  });

  it('leaves the parent alone when the field is absent', () => {
    upsertThread(threadMap.value, summary(undefined), false);
    expect(threadMap.value.get('child')?.meta.parentThreadId).toBe('parent');
  });
});
