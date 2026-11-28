import { describe, it, expect, vi } from 'vitest';

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

import { makeThreadState } from './threads-test-helpers';
import { setDrawerGrouping, setSelectedOngoingGroup, focusedThreadId, threadMap } from '../store';
import { focusThread, handleArchiveThread, rememberFocusedViewRows } from './threads';
import * as drawerOrder from '../../components/drawer/drawerOrder';
import { provideDrawerOrder } from '../drawerOrderHandle';

vi.mock('../../api/threads', () => ({
  fetchThreads: vi.fn(),
  fetchThreadEvents: vi.fn().mockResolvedValue({ events: [], currentAggregate: null }),
  fetchThreadMessages: vi.fn(),
  saveThread: vi.fn().mockResolvedValue(undefined),
  archiveThread: vi.fn().mockResolvedValue({ archived: [] }),
}));

vi.mock('../../components/chat/promptFocus', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../components/chat/promptFocus')>()),
  focusPromptNow: vi.fn(),
}));

describe('handleArchiveThread: a thread focused before the shell landed', () => {
  it('lands on the row below once the shell has taken the opened position', async () => {
    const waiting = (id: string, at: string) => makeThreadState(id, {
      meta: { id, title: id, channel: 'claude_code', updatedAt: at, status: 'waiting_for_user_answer', messageCount: 1, section: 'inbox' },
    });
    threadMap.value = new Map([
      ['a1', waiting('a1', '2026-01-01T00:00:03Z')],
      ['a2', waiting('a2', '2026-01-01T00:00:02Z')],
      ['a3', waiting('a3', '2026-01-01T00:00:01Z')],
    ]);
    setDrawerGrouping('ongoing');
    setSelectedOngoingGroup('blocked');
    focusThread('a2');

    provideDrawerOrder(drawerOrder);
    rememberFocusedViewRows();

    const stopped = new Map(threadMap.value);
    const a2 = stopped.get('a2')!;
    stopped.set('a2', { ...a2, meta: { ...a2.meta, status: 'idle' } });
    threadMap.value = stopped;

    await handleArchiveThread('a2');

    expect(focusedThreadId.value).toBe('a3');
  });
});
