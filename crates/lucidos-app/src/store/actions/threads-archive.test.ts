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
});

import { makeThreadState } from './threads-test-helpers';
import { type ThreadState } from '../thread-events';
import { archiveThread } from '../../api/threads';
import { putComposeOnThread } from '../../api/client';
import { focusPromptNow } from '../../components/chat/promptFocus';
import { drawerOpen } from '../../components/layout/drawerState';
import { _resetComposeDraftsForTesting, draftPresentThreadIds, getDraft } from '../composeDrafts';
import { ALL_CHANNELS, confirmState, setDrawerGrouping, setSelectedOngoingGroup, focusedThreadId, generatedTitleIds, getThreadDisplaySection, mobileView, selectedAppIds, selectedRepoIds, selectedTriggerIds, threadChannelFilter, threadDrawerOpen, threadMap, threadSearchQuery, threadSearchResults, toasts } from '../store';
import { upsertThread } from './thread-loading';
import { handleThreadEvent } from './thread-sync';
import { ARCHIVE_PINNED_CONFIRM, focusThread, handleArchiveThread, subscriptionsStoppedByArchive, unfocusThread } from './threads';
import { _resetRowIntentsForTesting, archiveRequestInFlight } from '../rowIntents';
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

vi.mock('../../api/client', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../api/client')>()),
  API_BASE: '',
  submitChat: vi.fn().mockResolvedValue(undefined),
  cancelChat: vi.fn(),
  stopClaudeCode: vi.fn(),
  putComposeOnThread: vi.fn().mockResolvedValue({ status: 'applied', report: null }),
  ensureThreadStarted: vi.fn().mockResolvedValue(undefined),
  deleteThread: vi.fn().mockResolvedValue(undefined),
}));


provideDrawerOrder(drawerOrder);

beforeEach(() => {
  threadMap.value = new Map();
  _resetComposeDraftsForTesting();
  focusedThreadId.value = null;
  (globalThis as any).innerWidth = 1024;
  mobileView.value = 'thread';
  threadDrawerOpen.value = false;
  drawerOpen.value = false;
  generatedTitleIds.clear();
  _resetRowIntentsForTesting();
  threadChannelFilter.value = new Set(ALL_CHANNELS);
  selectedTriggerIds.value = new Set();
  selectedRepoIds.value = new Set();
  selectedAppIds.value = new Set();
  setDrawerGrouping('folders');
  setSelectedOngoingGroup('blocked');
  threadSearchQuery.value = '';
  threadSearchResults.value = { status: 'not-loaded' };
  localStorage.removeItem('lucidos-focused-thread');
});

describe('handleArchiveThread', () => {
  it('focuses the next review thread after dismissing', async () => {
    const map = new Map<string, ThreadState>();
    map.set('t1', makeThreadState('t1', {
      meta: { id: 't1', title: 'Thread t1', channel: 'claude_code', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:01Z', status: 'waiting', codingAgentChangeState: { kind: 'proposed', requires_restart: false }, codingAgentIsExternalRepo: false, lastRevivedAt: '', messageCount: 1, section: 'inbox', activeChildrenCount: 0 },
    }));
    map.set('t2', makeThreadState('t2', {
      meta: { id: 't2', title: 'Thread t2', channel: 'claude_code', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:00Z', status: 'waiting', codingAgentChangeState: { kind: 'proposed', requires_restart: false }, codingAgentIsExternalRepo: false, lastRevivedAt: '', messageCount: 1, section: 'inbox', activeChildrenCount: 0 },
    }));
    threadMap.value = map;
    focusThread('t1');

    await handleArchiveThread('t1');

    expect(focusedThreadId.value).toBe('t2');
  });

  it('mobile: unfocuses but stays on the thread drawer when no more review threads remain', async () => {
    (globalThis as any).innerWidth = 375;
    const map = new Map<string, ThreadState>();
    map.set('t1', makeThreadState('t1', {
      meta: { id: 't1', title: 'Thread t1', channel: 'claude_code', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:00Z', status: 'waiting', codingAgentChangeState: { kind: 'proposed', requires_restart: false }, codingAgentIsExternalRepo: false, lastRevivedAt: '', messageCount: 1, section: 'inbox', activeChildrenCount: 0 },
    }));
    threadMap.value = map;
    focusThread('t1');
    mobileView.value = 'threads';

    await handleArchiveThread('t1');

    expect(focusedThreadId.value).toBeNull();
    expect(mobileView.value).toBe('threads');
  });

  it('mobile: stays on the thread drawer when the focus hands off to the next thread', async () => {
    (globalThis as any).innerWidth = 375;
    const map = new Map<string, ThreadState>();
    map.set('t1', makeThreadState('t1', {
      meta: { id: 't1', title: 'Thread t1', channel: 'claude_code', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:01Z', status: 'waiting', codingAgentChangeState: { kind: 'proposed', requires_restart: false }, codingAgentIsExternalRepo: false, lastRevivedAt: '', messageCount: 1, section: 'inbox', activeChildrenCount: 0 },
    }));
    map.set('t2', makeThreadState('t2', {
      meta: { id: 't2', title: 'Thread t2', channel: 'claude_code', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:00Z', status: 'waiting', codingAgentChangeState: { kind: 'proposed', requires_restart: false }, codingAgentIsExternalRepo: false, lastRevivedAt: '', messageCount: 1, section: 'inbox', activeChildrenCount: 0 },
    }));
    threadMap.value = map;
    focusThread('t1');
    mobileView.value = 'threads';

    await handleArchiveThread('t1');

    expect(focusedThreadId.value).toBe('t2');
    expect(mobileView.value).toBe('threads');
  });

  it('mobile: a rejected archive re-focuses without leaving the thread drawer', async () => {
    (globalThis as any).innerWidth = 375;
    const map = new Map<string, ThreadState>();
    map.set('t1', makeThreadState('t1', {
      meta: { id: 't1', title: 'Thread t1', channel: 'claude_code', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:01Z', status: 'waiting', codingAgentChangeState: { kind: 'proposed', requires_restart: false }, codingAgentIsExternalRepo: false, lastRevivedAt: '', messageCount: 1, section: 'inbox', activeChildrenCount: 0 },
    }));
    map.set('t2', makeThreadState('t2', {
      meta: { id: 't2', title: 'Thread t2', channel: 'claude_code', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:00Z', status: 'waiting', codingAgentChangeState: { kind: 'proposed', requires_restart: false }, codingAgentIsExternalRepo: false, lastRevivedAt: '', messageCount: 1, section: 'inbox', activeChildrenCount: 0 },
    }));
    threadMap.value = map;
    focusThread('t1');
    mobileView.value = 'threads';
    (archiveThread as ReturnType<typeof vi.fn>).mockRejectedValueOnce(new Error('nope'));

    await handleArchiveThread('t1');

    expect(focusedThreadId.value).toBe('t1');
    expect(mobileView.value).toBe('threads');
  });

  it('does not focus prompt when dismissing the last review thread', async () => {
    const map = new Map<string, ThreadState>();
    map.set('t1', makeThreadState('t1', {
      meta: { id: 't1', title: 'Last review', channel: 'claude_code', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:00Z', status: 'waiting', codingAgentChangeState: { kind: 'proposed', requires_restart: false }, codingAgentIsExternalRepo: false, lastRevivedAt: '', messageCount: 1, section: 'inbox', activeChildrenCount: 0 },
    }));
    threadMap.value = map;
    focusThread('t1');
    (focusPromptNow as ReturnType<typeof vi.fn>).mockClear();

    await handleArchiveThread('t1');

    expect(focusPromptNow).not.toHaveBeenCalled();
    expect(focusedThreadId.value).toBeNull();
  });

  it('focuses the next thread below, not the top one', async () => {
    const map = new Map<string, ThreadState>();
    map.set('t1', makeThreadState('t1', {
      meta: { id: 't1', title: 'Thread t1', channel: 'claude_code', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:02Z', status: 'waiting', codingAgentChangeState: { kind: 'none' }, codingAgentIsExternalRepo: false, lastRevivedAt: '', messageCount: 1, section: 'inbox', activeChildrenCount: 0 },
    }));
    map.set('t2', makeThreadState('t2', {
      meta: { id: 't2', title: 'Thread t2', channel: 'claude_code', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:01Z', status: 'waiting', codingAgentChangeState: { kind: 'none' }, codingAgentIsExternalRepo: false, lastRevivedAt: '', messageCount: 1, section: 'inbox', activeChildrenCount: 0 },
    }));
    map.set('t3', makeThreadState('t3', {
      meta: { id: 't3', title: 'Thread t3', channel: 'claude_code', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:00Z', status: 'waiting', codingAgentChangeState: { kind: 'none' }, codingAgentIsExternalRepo: false, lastRevivedAt: '', messageCount: 1, section: 'inbox', activeChildrenCount: 0 },
    }));
    threadMap.value = map;
    focusThread('t2');

    await handleArchiveThread('t2');

    expect(focusedThreadId.value).toBe('t3');
  });

  it('picks the next focus from the rows as they stood before the archive flipped the row out', async () => {
    const meta = (id: string, updatedAt: string) => ({ id, title: `Thread ${id}`, channel: 'claude_code' as const, saved: false, createdAt: '', updatedAt, status: 'waiting' as const, codingAgentChangeState: { kind: 'none' as const }, codingAgentIsExternalRepo: false, lastRevivedAt: '', messageCount: 1, section: 'inbox' as const, activeChildrenCount: 0 });
    threadMap.value = new Map([
      ['t1', makeThreadState('t1', { meta: meta('t1', '2026-01-01T00:00:02Z') })],
      ['t2', makeThreadState('t2', { meta: meta('t2', '2026-01-01T00:00:01Z') })],
      ['t3', makeThreadState('t3', { meta: meta('t3', '2026-01-01T00:00:00Z') })],
    ]);
    unfocusThread({ revealPane: false });
    focusedThreadId.value = 't2';

    await handleArchiveThread('t2');

    expect(focusedThreadId.value).toBe('t3');
  });

  it('skips threads hidden by the active channel filter when picking the next focus', async () => {
    const map = new Map<string, ThreadState>();
    map.set('t1', makeThreadState('t1', {
      meta: { id: 't1', title: 'CC t1', channel: 'claude_code', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:02Z', status: 'waiting', codingAgentChangeState: { kind: 'proposed', requires_restart: false }, codingAgentIsExternalRepo: false, lastRevivedAt: '', messageCount: 1, section: 'inbox', activeChildrenCount: 0 },
    }));
    map.set('t2', makeThreadState('t2', {
      meta: { id: 't2', title: 'Chat t2', channel: 'chat', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:01Z', status: 'failed', codingAgentChangeState: { kind: 'none' }, codingAgentIsExternalRepo: false, lastRevivedAt: '', messageCount: 1, section: 'inbox', activeChildrenCount: 0 },
    }));
    map.set('t3', makeThreadState('t3', {
      meta: { id: 't3', title: 'CC t3', channel: 'claude_code', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:00Z', status: 'waiting', codingAgentChangeState: { kind: 'proposed', requires_restart: false }, codingAgentIsExternalRepo: false, lastRevivedAt: '', messageCount: 1, section: 'inbox', activeChildrenCount: 0 },
    }));
    threadMap.value = map;
    threadChannelFilter.value = new Set(['claude_code']);
    focusThread('t1');

    await handleArchiveThread('t1');

    expect(focusedThreadId.value).toBe('t3');
  });

  it('navigates to next review thread after apply+archive (regression: Apply used to skip Archive)', async () => {
    const map = new Map<string, ThreadState>();
    map.set('t1', makeThreadState('t1', {
      meta: { id: 't1', title: 'Applied thread', channel: 'claude_code', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:01Z', status: 'idle', codingAgentChangeState: { kind: 'none' }, codingAgentIsExternalRepo: false, lastRevivedAt: '', messageCount: 1, section: 'inbox', activeChildrenCount: 0 },
    }));
    map.set('t2', makeThreadState('t2', {
      meta: { id: 't2', title: 'Pending thread', channel: 'claude_code', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:00Z', status: 'waiting', codingAgentChangeState: { kind: 'proposed', requires_restart: false }, codingAgentIsExternalRepo: false, lastRevivedAt: '', messageCount: 1, section: 'inbox', activeChildrenCount: 0 },
    }));
    threadMap.value = map;
    focusThread('t1');

    await handleArchiveThread('t1');

    expect(focusedThreadId.value).toBe('t2');
  });

  it('keeps thread drawer open on desktop when all reviews are done (compose view)', async () => {
    const map = new Map<string, ThreadState>();
    map.set('t1', makeThreadState('t1', {
      meta: { id: 't1', title: 'Last review', channel: 'claude_code', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:00Z', status: 'waiting', codingAgentChangeState: { kind: 'proposed', requires_restart: false }, codingAgentIsExternalRepo: false, lastRevivedAt: '', messageCount: 1, section: 'inbox', activeChildrenCount: 0 },
    }));
    threadMap.value = map;
    focusThread('t1');
    threadDrawerOpen.value = true;

    await handleArchiveThread('t1');

    expect(focusedThreadId.value).toBeNull();
    expect(threadDrawerOpen.value).toBe(true);
  });

  it('focuses the thread above when dismissing the last item', async () => {
    const map = new Map<string, ThreadState>();
    map.set('t1', makeThreadState('t1', {
      meta: { id: 't1', title: 'Thread t1', channel: 'claude_code', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:01Z', status: 'waiting', codingAgentChangeState: { kind: 'none' }, codingAgentIsExternalRepo: false, lastRevivedAt: '', messageCount: 1, section: 'inbox', activeChildrenCount: 0 },
    }));
    map.set('t2', makeThreadState('t2', {
      meta: { id: 't2', title: 'Thread t2', channel: 'claude_code', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:00Z', status: 'waiting', codingAgentChangeState: { kind: 'none' }, codingAgentIsExternalRepo: false, lastRevivedAt: '', messageCount: 1, section: 'inbox', activeChildrenCount: 0 },
    }));
    threadMap.value = map;
    focusThread('t2');

    await handleArchiveThread('t2');

    expect(focusedThreadId.value).toBe('t1');
  });

  it('skips cascaded descendants and lands on the next non-archived review thread', async () => {
    const map = new Map<string, ThreadState>();
    map.set('parent', makeThreadState('parent', {
      meta: { id: 'parent', title: 'Parent', channel: 'claude_code', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:02Z', status: 'waiting', codingAgentChangeState: { kind: 'proposed', requires_restart: false }, codingAgentIsExternalRepo: false, lastRevivedAt: '', messageCount: 1, section: 'inbox', activeChildrenCount: 0 },
    }));
    map.set('child', makeThreadState('child', {
      meta: { id: 'child', title: 'Child', channel: 'claude_code', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:01Z', status: 'waiting', codingAgentChangeState: { kind: 'proposed', requires_restart: false }, codingAgentIsExternalRepo: false, lastRevivedAt: '', messageCount: 1, section: 'inbox', activeChildrenCount: 0, parentThreadId: 'parent' },
    }));
    map.set('sibling', makeThreadState('sibling', {
      meta: { id: 'sibling', title: 'Sibling', channel: 'claude_code', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:00Z', status: 'waiting', codingAgentChangeState: { kind: 'proposed', requires_restart: false }, codingAgentIsExternalRepo: false, lastRevivedAt: '', messageCount: 1, section: 'inbox', activeChildrenCount: 0 },
    }));
    threadMap.value = map;
    focusThread('parent');

    await handleArchiveThread('parent');

    expect(focusedThreadId.value).toBe('sibling');
  });

  it('lands on the next family parent, not a sub-thread that sorts high by its own review tier', async () => {
    const map = new Map<string, ThreadState>();
    map.set('pA', makeThreadState('pA', {
      meta: { id: 'pA', title: 'Family A parent', channel: 'claude_code', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:03Z', status: 'waiting', codingAgentChangeState: { kind: 'proposed', requires_restart: false }, codingAgentIsExternalRepo: false, lastRevivedAt: '', messageCount: 1, section: 'inbox', activeChildrenCount: 0 },
    }));
    map.set('pB', makeThreadState('pB', {
      meta: { id: 'pB', title: 'Family B parent', channel: 'claude_code', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:01Z', status: 'running', codingAgentChangeState: { kind: 'none' }, codingAgentIsExternalRepo: false, lastRevivedAt: '', messageCount: 1, section: 'inbox', activeChildrenCount: 1 },
    }));
    map.set('cB', makeThreadState('cB', {
      meta: { id: 'cB', title: 'Family B sub-thread', channel: 'claude_code', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:02Z', status: 'waiting', codingAgentChangeState: { kind: 'proposed', requires_restart: false }, codingAgentIsExternalRepo: false, lastRevivedAt: '', messageCount: 1, section: 'inbox', activeChildrenCount: 0, parentThreadId: 'pB' },
    }));
    threadMap.value = map;
    focusThread('pA');

    await handleArchiveThread('pA');

    expect(focusedThreadId.value).toBe('pB');
  });

  it('skips discarded inbox orphans when picking the next focus', async () => {
    const map = new Map<string, ThreadState>();
    map.set('focused', makeThreadState('focused', {
      meta: { updatedAt: '2026-01-01T00:00:03Z', messageCount: 1, section: 'inbox' },
    }));
    map.set('orphan', makeThreadState('orphan', {
      meta: { updatedAt: '2026-01-01T00:00:02Z', section: 'inbox', state: 'discarded', channel: 'claude_code' },
    }));
    map.set('sibling', makeThreadState('sibling', {
      meta: { updatedAt: '2026-01-01T00:00:01Z', messageCount: 1, section: 'inbox' },
    }));
    threadMap.value = map;
    focusThread('focused');

    await handleArchiveThread('focused');

    expect(focusedThreadId.value).toBe('sibling');
  });

  it('skips composing drafts when picking the next focus', async () => {
    const map = new Map<string, ThreadState>();
    map.set('focused', makeThreadState('focused', {
      meta: { updatedAt: '2026-01-01T00:00:03Z', channel: 'claude_code', status: 'waiting', codingAgentChangeState: { kind: 'proposed', requires_restart: false }, messageCount: 1, section: 'inbox' },
    }));
    map.set('draft', makeThreadState('draft', {
      meta: { updatedAt: '2026-01-01T00:00:02Z', channel: 'claude_code', section: 'inbox', state: 'composing' },
    }));
    map.set('sibling', makeThreadState('sibling', {
      meta: { updatedAt: '2026-01-01T00:00:01Z', channel: 'claude_code', status: 'waiting', codingAgentChangeState: { kind: 'proposed', requires_restart: false }, messageCount: 1, section: 'inbox' },
    }));
    threadMap.value = map;
    focusThread('focused');

    await handleArchiveThread('focused');

    expect(focusedThreadId.value).toBe('sibling');
  });

  it('cascading archive still lands on a real review sibling, not a discarded orphan', async () => {
    const map = new Map<string, ThreadState>();
    map.set('parent', makeThreadState('parent', {
      meta: { updatedAt: '2026-01-01T00:00:04Z', channel: 'claude_code', status: 'waiting', codingAgentChangeState: { kind: 'proposed', requires_restart: false }, messageCount: 1, section: 'inbox' },
    }));
    map.set('child', makeThreadState('child', {
      meta: { updatedAt: '2026-01-01T00:00:03Z', channel: 'claude_code', status: 'waiting', codingAgentChangeState: { kind: 'proposed', requires_restart: false }, messageCount: 1, section: 'inbox', parentThreadId: 'parent' },
    }));
    map.set('orphan', makeThreadState('orphan', {
      meta: { updatedAt: '2026-01-01T00:00:02Z', channel: 'claude_code', section: 'inbox', state: 'discarded' },
    }));
    map.set('sibling', makeThreadState('sibling', {
      meta: { updatedAt: '2026-01-01T00:00:01Z', channel: 'claude_code', status: 'waiting', codingAgentChangeState: { kind: 'proposed', requires_restart: false }, messageCount: 1, section: 'inbox' },
    }));
    threadMap.value = map;
    focusThread('parent');

    await handleArchiveThread('parent');

    expect(focusedThreadId.value).toBe('sibling');
  });

  it('falls back to unfocus when the only remaining review threads are discarded orphans', async () => {
    const map = new Map<string, ThreadState>();
    map.set('focused', makeThreadState('focused', {
      meta: { updatedAt: '2026-01-01T00:00:02Z', messageCount: 1, section: 'inbox' },
    }));
    map.set('orphan', makeThreadState('orphan', {
      meta: { updatedAt: '2026-01-01T00:00:01Z', channel: 'claude_code', section: 'inbox', state: 'discarded' },
    }));
    threadMap.value = map;
    focusThread('focused');

    await handleArchiveThread('focused');

    expect(focusedThreadId.value).toBeNull();
  });
});


describe('handleArchiveThread — the list on screen', () => {
  it('lands on the next Blocked thread, skipping a Current-only thread', async () => {
    const map = new Map<string, ThreadState>();
    map.set('a1', makeThreadState('a1', {
      meta: { id: 'a1', title: 'Attention 1', channel: 'claude_code', updatedAt: '2026-01-01T00:00:03Z', status: 'waiting_for_user_answer', messageCount: 1, section: 'inbox' },
    }));
    map.set('c2', makeThreadState('c2', {
      meta: { id: 'c2', title: 'Current only', channel: 'claude_code', updatedAt: '2026-01-01T00:00:02Z', status: 'idle', messageCount: 1, section: 'inbox' },
    }));
    map.set('a3', makeThreadState('a3', {
      meta: { id: 'a3', title: 'Attention 3', channel: 'claude_code', updatedAt: '2026-01-01T00:00:01Z', status: 'waiting_for_user_answer', messageCount: 1, section: 'inbox' },
    }));
    threadMap.value = map;
    setDrawerGrouping('ongoing');
    setSelectedOngoingGroup('blocked');
    focusThread('a1');

    await handleArchiveThread('a1');

    expect(focusedThreadId.value).toBe('a3');
  });

  it('lands on the next Review thread when the review view is active', async () => {
    const map = new Map<string, ThreadState>();
    map.set('r1', makeThreadState('r1', {
      meta: { id: 'r1', title: 'Review 1', channel: 'claude_code', updatedAt: '2026-01-01T00:00:03Z', status: 'idle', codingAgentChangeState: { kind: 'proposed', requires_restart: false }, messageCount: 1, section: 'inbox' },
    }));
    map.set('a2', makeThreadState('a2', {
      meta: { id: 'a2', title: 'Attention 2', channel: 'claude_code', updatedAt: '2026-01-01T00:00:02Z', status: 'waiting_for_user_answer', messageCount: 1, section: 'inbox' },
    }));
    map.set('r3', makeThreadState('r3', {
      meta: { id: 'r3', title: 'Review 3', channel: 'claude_code', updatedAt: '2026-01-01T00:00:01Z', status: 'idle', codingAgentChangeState: { kind: 'proposed', requires_restart: false }, messageCount: 1, section: 'inbox' },
    }));
    threadMap.value = map;
    setDrawerGrouping('ongoing');
    setSelectedOngoingGroup('review');
    focusThread('r1');

    await handleArchiveThread('r1');

    expect(focusedThreadId.value).toBe('r3');
  });

  it('lands on a Review thread when the archived thread already left the review view', async () => {
    const map = new Map<string, ThreadState>();
    map.set('r1', makeThreadState('r1', {
      meta: { id: 'r1', title: 'Applied', channel: 'claude_code', updatedAt: '2026-01-01T00:00:03Z', status: 'idle', codingAgentChangeState: { kind: 'none' }, messageCount: 1, section: 'inbox' },
    }));
    map.set('a2', makeThreadState('a2', {
      meta: { id: 'a2', title: 'Attention 2', channel: 'claude_code', updatedAt: '2026-01-01T00:00:02Z', status: 'waiting_for_user_answer', messageCount: 1, section: 'inbox' },
    }));
    map.set('r3', makeThreadState('r3', {
      meta: { id: 'r3', title: 'Review 3', channel: 'claude_code', updatedAt: '2026-01-01T00:00:01Z', status: 'idle', codingAgentChangeState: { kind: 'proposed', requires_restart: false }, messageCount: 1, section: 'inbox' },
    }));
    threadMap.value = map;
    setDrawerGrouping('ongoing');
    setSelectedOngoingGroup('review');
    focusThread('r1');

    await handleArchiveThread('r1');

    expect(focusedThreadId.value).toBe('r3');
  });

  it('walks the In flight view in its nested order, sub-threads under their parent', async () => {
    const map = new Map<string, ThreadState>();
    map.set('p1', makeThreadState('p1', {
      meta: { id: 'p1', title: 'Parent', channel: 'claude_code', updatedAt: '2026-01-01T00:00:04Z', status: 'idle', activeChildrenCount: 1, messageCount: 1, section: 'inbox' },
    }));
    map.set('c', makeThreadState('c', {
      meta: { id: 'c', title: 'Sub-thread', channel: 'claude_code', updatedAt: '2026-01-01T00:00:05Z', status: 'running', parentThreadId: 'p1', messageCount: 1, section: 'inbox' },
    }));
    map.set('x', makeThreadState('x', {
      meta: { id: 'x', title: 'Running', channel: 'claude_code', updatedAt: '2026-01-01T00:00:03Z', status: 'running', messageCount: 1, section: 'inbox' },
    }));
    threadMap.value = map;
    setDrawerGrouping('ongoing');
    setSelectedOngoingGroup('in-flight');
    focusThread('x');

    await handleArchiveThread('x');

    expect(focusedThreadId.value).toBe('c');
  });

  it('falls back to the thread above within the attention view', async () => {
    const map = new Map<string, ThreadState>();
    map.set('a1', makeThreadState('a1', {
      meta: { id: 'a1', title: 'Attention 1', channel: 'claude_code', updatedAt: '2026-01-01T00:00:02Z', status: 'waiting_for_user_answer', messageCount: 1, section: 'inbox' },
    }));
    map.set('a2', makeThreadState('a2', {
      meta: { id: 'a2', title: 'Attention 2', channel: 'claude_code', updatedAt: '2026-01-01T00:00:01Z', status: 'waiting_for_user_answer', messageCount: 1, section: 'inbox' },
    }));
    threadMap.value = map;
    setDrawerGrouping('ongoing');
    setSelectedOngoingGroup('blocked');
    focusThread('a2');

    await handleArchiveThread('a2');

    expect(focusedThreadId.value).toBe('a1');
  });

  it('lands on the row below when the archived thread was stopped out of the attention view first', async () => {
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

    const stopped = new Map(threadMap.value);
    const a2 = stopped.get('a2')!;
    stopped.set('a2', { ...a2, meta: { ...a2.meta, status: 'idle' } });
    threadMap.value = stopped;

    await handleArchiveThread('a2');

    expect(focusedThreadId.value).toBe('a3');
  });

  it('unfocuses when the attention view has no other thread (a Current-only thread is not offered)', async () => {
    const map = new Map<string, ThreadState>();
    map.set('a1', makeThreadState('a1', {
      meta: { id: 'a1', title: 'Attention 1', channel: 'claude_code', updatedAt: '2026-01-01T00:00:02Z', status: 'waiting_for_user_answer', messageCount: 1, section: 'inbox' },
    }));
    map.set('c2', makeThreadState('c2', {
      meta: { id: 'c2', title: 'Current only', channel: 'claude_code', updatedAt: '2026-01-01T00:00:01Z', status: 'idle', messageCount: 1, section: 'inbox' },
    }));
    threadMap.value = map;
    setDrawerGrouping('ongoing');
    setSelectedOngoingGroup('blocked');
    focusThread('a1');

    await handleArchiveThread('a1');

    expect(focusedThreadId.value).toBeNull();
  });

  it('walks the search results when a search query is active (overriding the view)', async () => {
    const map = new Map<string, ThreadState>();
    map.set('s1', makeThreadState('s1', {
      meta: { id: 's1', title: 'Search 1', channel: 'claude_code', updatedAt: '2026-01-01T00:00:03Z', status: 'waiting', codingAgentChangeState: { kind: 'proposed', requires_restart: false }, messageCount: 1, section: 'inbox' },
    }));
    map.set('s2', makeThreadState('s2', {
      meta: { id: 's2', title: 'Search 2', channel: 'claude_code', updatedAt: '2026-01-01T00:00:02Z', status: 'waiting', codingAgentChangeState: { kind: 'proposed', requires_restart: false }, messageCount: 1, section: 'inbox' },
    }));
    threadMap.value = map;
    setDrawerGrouping('folders');
    threadSearchQuery.value = 'foo';
    threadSearchResults.value = {
      status: 'loaded',
      data: [
        { thread_id: 's1' } as any,
        { thread_id: 's2' } as any,
      ],
    };
    focusThread('s1');

    await handleArchiveThread('s1');

    expect(focusedThreadId.value).toBe('s2');
  });
});


describe('handleArchiveThread — optimistic UI', () => {
  function currentSection(id: string): boolean {
    const thread = threadMap.value.get(id);
    return !!thread && getThreadDisplaySection(thread) === 'current';
  }

  it('drops the target out of review before the API resolves', async () => {
    let resolveApi: (v: { archived: string[] }) => void = () => {};
    (archiveThread as ReturnType<typeof vi.fn>).mockImplementationOnce(
      () => new Promise<{ archived: string[] }>(r => { resolveApi = r; }),
    );

    const map = new Map<string, ThreadState>();
    map.set('parent', makeThreadState('parent', {
      meta: { id: 'parent', title: 'Parent', channel: 'claude_code', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:01Z', status: 'waiting', codingAgentChangeState: { kind: 'proposed', requires_restart: false }, codingAgentIsExternalRepo: false, lastRevivedAt: '', messageCount: 1, section: 'inbox', activeChildrenCount: 0 },
    }));
    map.set('sibling', makeThreadState('sibling', {
      meta: { id: 'sibling', title: 'Sibling', channel: 'claude_code', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:00Z', status: 'waiting', codingAgentChangeState: { kind: 'proposed', requires_restart: false }, codingAgentIsExternalRepo: false, lastRevivedAt: '', messageCount: 1, section: 'inbox', activeChildrenCount: 0 },
    }));
    threadMap.value = map;
    focusThread('parent');

    const pending = handleArchiveThread('parent');

    expect(currentSection('parent')).toBe(false);
    expect(focusedThreadId.value).toBe('sibling');

    resolveApi({ archived: ['parent'] });
    await pending;
  });

  it('drops descendants out of review based on local parentThreadId', async () => {
    let resolveApi: (v: { archived: string[] }) => void = () => {};
    (archiveThread as ReturnType<typeof vi.fn>).mockImplementationOnce(
      () => new Promise<{ archived: string[] }>(r => { resolveApi = r; }),
    );

    const map = new Map<string, ThreadState>();
    map.set('parent', makeThreadState('parent', {
      meta: { id: 'parent', title: 'Parent', channel: 'claude_code', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:02Z', status: 'waiting', codingAgentChangeState: { kind: 'proposed', requires_restart: false }, codingAgentIsExternalRepo: false, lastRevivedAt: '', messageCount: 1, section: 'inbox', activeChildrenCount: 0 },
    }));
    map.set('child', makeThreadState('child', {
      meta: { id: 'child', title: 'Child', channel: 'claude_code', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:01Z', status: 'waiting', codingAgentChangeState: { kind: 'proposed', requires_restart: false }, codingAgentIsExternalRepo: false, lastRevivedAt: '', messageCount: 1, section: 'inbox', activeChildrenCount: 0, parentThreadId: 'parent' },
    }));
    map.set('grandchild', makeThreadState('grandchild', {
      meta: { id: 'grandchild', title: 'Grandchild', channel: 'claude_code', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:00Z', status: 'waiting', codingAgentChangeState: { kind: 'proposed', requires_restart: false }, codingAgentIsExternalRepo: false, lastRevivedAt: '', messageCount: 1, section: 'inbox', activeChildrenCount: 0, parentThreadId: 'child' },
    }));
    threadMap.value = map;
    focusThread('parent');

    const pending = handleArchiveThread('parent');

    for (const id of ['parent', 'child', 'grandchild']) {
      expect(currentSection(id), id).toBe(false);
    }

    resolveApi({ archived: ['parent', 'child', 'grandchild'] });
    await pending;
  });

  it('leaves the user where they navigated to if archive rejects mid-flight', async () => {
    let rejectApi: (e: Error) => void = () => {};
    (archiveThread as ReturnType<typeof vi.fn>).mockImplementationOnce(
      () => new Promise<{ archived: string[] }>((_resolve, reject) => { rejectApi = reject; }),
    );

    const map = new Map<string, ThreadState>();
    map.set('parent', makeThreadState('parent', {
      meta: { id: 'parent', title: 'Parent', channel: 'claude_code', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:02Z', status: 'waiting', codingAgentChangeState: { kind: 'proposed', requires_restart: false }, codingAgentIsExternalRepo: false, lastRevivedAt: '', messageCount: 1, section: 'inbox', activeChildrenCount: 0 },
    }));
    map.set('sibling', makeThreadState('sibling', {
      meta: { id: 'sibling', title: 'Sibling', channel: 'claude_code', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:01Z', status: 'waiting', codingAgentChangeState: { kind: 'proposed', requires_restart: false }, codingAgentIsExternalRepo: false, lastRevivedAt: '', messageCount: 1, section: 'inbox', activeChildrenCount: 0 },
    }));
    map.set('elsewhere', makeThreadState('elsewhere', {
      meta: { id: 'elsewhere', title: 'Elsewhere', channel: 'chat', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:00Z', status: 'idle', codingAgentChangeState: { kind: 'none' }, codingAgentIsExternalRepo: false, lastRevivedAt: '', messageCount: 0, section: 'archived', activeChildrenCount: 0 },
    }));
    threadMap.value = map;
    focusThread('parent');

    const pending = handleArchiveThread('parent');
    expect(focusedThreadId.value).toBe('sibling');

    focusThread('elsewhere');
    expect(focusedThreadId.value).toBe('elsewhere');

    rejectApi(new Error('boom'));
    await pending;

    expect(focusedThreadId.value).toBe('elsewhere');
  });

  it('restores section + change state and shows a toast when the API rejects', async () => {
    (archiveThread as ReturnType<typeof vi.fn>).mockRejectedValueOnce(new Error('boom'));

    const map = new Map<string, ThreadState>();
    map.set('parent', makeThreadState('parent', {
      meta: { id: 'parent', title: 'Parent', channel: 'claude_code', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:01Z', status: 'waiting', codingAgentChangeState: { kind: 'proposed', requires_restart: false }, codingAgentIsExternalRepo: false, lastRevivedAt: '', messageCount: 1, section: 'inbox', activeChildrenCount: 0 },
    }));
    map.set('child', makeThreadState('child', {
      meta: { id: 'child', title: 'Child', channel: 'claude_code', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:00Z', status: 'waiting', codingAgentChangeState: { kind: 'proposed', requires_restart: false }, codingAgentIsExternalRepo: false, lastRevivedAt: '', messageCount: 1, section: 'inbox', activeChildrenCount: 0, parentThreadId: 'parent' },
    }));
    threadMap.value = map;
    focusThread('parent');
    toasts.value = [];

    await handleArchiveThread('parent');

    expect(threadMap.value.get('parent')?.meta.section).toBe('inbox');
    expect(threadMap.value.get('child')?.meta.section).toBe('inbox');
    expect(threadMap.value.get('parent')?.meta.codingAgentChangeState.kind).toBe('proposed');
    expect(threadMap.value.get('child')?.meta.codingAgentChangeState.kind).toBe('proposed');
    expect(archiveRequestInFlight('parent')).toBe(false);
    expect(toasts.value.some(t => t.type === 'error')).toBe(true);
  });
});

describe('handleArchiveThread: a pinned thread', () => {
  function seedPinned(): void {
    threadMap.value = new Map([
      ['p', makeThreadState('p', { meta: { title: 'Pinned', saved: true, section: 'inbox', status: 'idle', messageCount: 1 } })],
    ]);
    focusThread('p');
    confirmState.value = { visible: false, message: '', okLabel: 'Archive' };
  }

  it('says that archiving unpins, then moves the row from Pinned to Archive at once', async () => {
    seedPinned();
    let resolveApi: (v: { archived: string[] }) => void = () => {};
    (archiveThread as ReturnType<typeof vi.fn>).mockImplementationOnce(
      () => new Promise<{ archived: string[] }>(r => { resolveApi = r; }),
    );

    const pending = handleArchiveThread('p');
    expect(confirmState.value.visible).toBe(true);
    expect(confirmState.value.message).toBe(ARCHIVE_PINNED_CONFIRM);
    confirmState.value.resolve?.(true);
    await vi.waitFor(() => expect(archiveThread).toHaveBeenCalledWith('p'));

    const thread = threadMap.value.get('p')!;
    expect(thread.meta.saved).toBe(false);
    expect(thread.meta.section).toBe('archived');
    expect(getThreadDisplaySection(thread)).toBe('archive');

    resolveApi({ archived: ['p'] });
    await pending;
  });

  it('stays pinned in the inbox when the user cancels the confirm', async () => {
    seedPinned();
    (archiveThread as ReturnType<typeof vi.fn>).mockClear();

    const pending = handleArchiveThread('p');
    confirmState.value.resolve?.(false);
    await pending;

    expect(archiveThread).not.toHaveBeenCalled();
    expect(threadMap.value.get('p')!.meta).toMatchObject({ saved: true, section: 'inbox' });
  });

  it('restores the pin when the archive is rejected', async () => {
    seedPinned();
    (archiveThread as ReturnType<typeof vi.fn>).mockRejectedValueOnce(new Error('boom'));

    const pending = handleArchiveThread('p');
    confirmState.value.resolve?.(true);
    await pending;

    expect(threadMap.value.get('p')!.meta).toMatchObject({ saved: true, section: 'inbox' });
  });

  it('keeps the unpin when a GET issued before the archive lands with the old pin', async () => {
    seedPinned();
    let resolveApi: (v: { archived: string[] }) => void = () => {};
    (archiveThread as ReturnType<typeof vi.fn>).mockImplementationOnce(
      () => new Promise<{ archived: string[] }>(r => { resolveApi = r; }),
    );
    await new Promise(r => setTimeout(r, 5));

    const pending = handleArchiveThread('p');
    confirmState.value.resolve?.(true);
    await vi.waitFor(() => expect(threadMap.value.get('p')!.meta.section).toBe('archived'));

    upsertThread(threadMap.value, {
      thread_id: 'p',
      title: 'Pinned',
      channel: 'chat',
      last_activity: '2026-01-01T00:00:00Z',
      created_at: '',
      message_count: 1,
      section: 'inbox',
      status: 'idle',
    } as any, true);

    expect(threadMap.value.get('p')!.meta).toMatchObject({ saved: false, section: 'archived' });

    resolveApi({ archived: ['p'] });
    await pending;
  });
});


describe('handleArchiveThread — stale loadAllThreads response', () => {
  it('keeps section archived when a GET issued before the flip lands after with the pre-archive snapshot', async () => {
    let resolveApi: (v: { archived: string[] }) => void = () => {};
    (archiveThread as ReturnType<typeof vi.fn>).mockImplementationOnce(
      () => new Promise<{ archived: string[] }>(r => { resolveApi = r; }),
    );

    const map = new Map<string, ThreadState>();
    map.set('parent', makeThreadState('parent', {
      meta: { id: 'parent', title: 'Parent', channel: 'claude_code', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:01Z', status: 'waiting', codingAgentChangeState: { kind: 'proposed', requires_restart: false }, codingAgentIsExternalRepo: false, lastRevivedAt: '', messageCount: 1, section: 'inbox', activeChildrenCount: 0 },
    }));
    map.set('sibling', makeThreadState('sibling', {
      meta: { id: 'sibling', title: 'Sibling', channel: 'chat', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:00Z', status: 'idle', codingAgentChangeState: { kind: 'none' }, codingAgentIsExternalRepo: false, lastRevivedAt: '', messageCount: 1, section: 'inbox', activeChildrenCount: 0 },
    }));
    threadMap.value = map;
    focusThread('parent');


    const pending = handleArchiveThread('parent');
    expect(threadMap.value.get('parent')?.meta.section).toBe('archived');
    expect(threadMap.value.get('parent')?.meta.codingAgentChangeState.kind).toBe('none');

    upsertThread(threadMap.value, {
      thread_id: 'parent',
      title: 'Parent',
      channel: 'claude_code',
      last_activity: '2026-01-01T00:00:01Z',
      created_at: '',
      message_count: 1,
      section: 'inbox',
      status: 'waiting',
      coding_agent_change_state: { kind: 'proposed', requires_restart: false },
    } as any, false);

    expect(threadMap.value.get('parent')?.meta.section).toBe('archived');
    expect(threadMap.value.get('parent')?.meta.codingAgentChangeState.kind).toBe('none');

    resolveApi({ archived: ['parent'] });
    await pending;
  });

  it('marks every cascade descendant so a stale GET cannot resurrect a child either', async () => {
    let resolveApi: (v: { archived: string[] }) => void = () => {};
    (archiveThread as ReturnType<typeof vi.fn>).mockImplementationOnce(
      () => new Promise<{ archived: string[] }>(r => { resolveApi = r; }),
    );

    const map = new Map<string, ThreadState>();
    map.set('parent', makeThreadState('parent', {
      meta: { id: 'parent', title: 'Parent', channel: 'claude_code', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:02Z', status: 'waiting', codingAgentChangeState: { kind: 'proposed', requires_restart: false }, codingAgentIsExternalRepo: false, lastRevivedAt: '', messageCount: 1, section: 'inbox', activeChildrenCount: 0 },
    }));
    map.set('child', makeThreadState('child', {
      meta: { id: 'child', title: 'Child', channel: 'claude_code', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:01Z', status: 'waiting', codingAgentChangeState: { kind: 'proposed', requires_restart: false }, codingAgentIsExternalRepo: false, lastRevivedAt: '', messageCount: 1, section: 'inbox', activeChildrenCount: 0, parentThreadId: 'parent' },
    }));
    threadMap.value = map;
    focusThread('parent');

    await new Promise(r => setTimeout(r, 5));

    const pending = handleArchiveThread('parent');

    upsertThread(threadMap.value, {
      thread_id: 'child',
      title: 'Child',
      channel: 'claude_code',
      last_activity: '2026-01-01T00:00:01Z',
      created_at: '',
      message_count: 1,
      section: 'inbox',
      status: 'waiting',
      coding_agent_change_state: { kind: 'proposed', requires_restart: false },
      parent_thread_id: 'parent',
    } as any, false);

    expect(threadMap.value.get('child')?.meta.section).toBe('archived');
    expect(threadMap.value.get('child')?.meta.codingAgentChangeState.kind).toBe('none');

    resolveApi({ archived: ['parent', 'child'] });
    await pending;
  });

  it('does not block a fresh GET issued after the flip from updating section', async () => {
    let resolveApi: (v: { archived: string[] }) => void = () => {};
    (archiveThread as ReturnType<typeof vi.fn>).mockImplementationOnce(
      () => new Promise<{ archived: string[] }>(r => { resolveApi = r; }),
    );

    const map = new Map<string, ThreadState>();
    map.set('parent', makeThreadState('parent', {
      meta: { id: 'parent', title: 'Parent', channel: 'chat', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:01Z', status: 'idle', codingAgentChangeState: { kind: 'none' }, codingAgentIsExternalRepo: false, lastRevivedAt: '', messageCount: 1, section: 'inbox', activeChildrenCount: 0 },
    }));
    threadMap.value = map;
    focusThread('parent');

    const pending = handleArchiveThread('parent');
    expect(threadMap.value.get('parent')?.meta.section).toBe('archived');

    await new Promise(r => setTimeout(r, 5));

    upsertThread(threadMap.value, {
      thread_id: 'parent',
      title: 'Parent',
      channel: 'chat',
      last_activity: '2026-01-01T00:00:01Z',
      created_at: '',
      message_count: 1,
      section: 'archived',
      status: 'idle',
      coding_agent_change_state: { kind: 'none' },
    } as any, false);

    expect(threadMap.value.get('parent')?.meta.section).toBe('archived');

    resolveApi({ archived: ['parent'] });
    await pending;
  });
});


describe('handleArchiveThread — stale SSE aggregate', () => {
  it('keeps section archived when an SSE event arrives with a pre-archive aggregate', async () => {
    let resolveApi: (v: { archived: string[] }) => void = () => {};
    (archiveThread as ReturnType<typeof vi.fn>).mockImplementationOnce(
      () => new Promise<{ archived: string[] }>(r => { resolveApi = r; }),
    );

    const map = new Map<string, ThreadState>();
    map.set('t', makeThreadState('t', {
      meta: { id: 't', title: 'CC thread', channel: 'claude_code', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:01Z', status: 'idle', codingAgentChangeState: { kind: 'none' }, codingAgentIsExternalRepo: false, lastRevivedAt: '', messageCount: 1, section: 'inbox', activeChildrenCount: 0 },
    }));
    threadMap.value = map;
    focusThread('t');

    const pending = handleArchiveThread('t');
    expect(threadMap.value.get('t')?.meta.section).toBe('archived');

    handleThreadEvent({
      thread_id: 't',
      seq: 101,
      created: '2026-01-01T00:00:02Z',
      event: { type: 'CodingAgentIdled' },
      aggregate: {
        threadId: 't',
        title: 'CC thread',
        channel: 'claude_code',
        initiator: 'user',
        createdAt: '',
        lastActivity: '2026-01-01T00:00:02Z',
        messageCount: 1,
        section: 'inbox',
        status: 'idle',
        activeChildrenCount: 0,
        totalChildrenCount: 0,
        blockingDescendantCount: 0, attentionDescendantCount: 0,
        codingAgentChangeState: { kind: 'none' },
        codingAgentIsExternalRepo: false,
        isSaved: false,
        hasResponse: true,
        lastRevivedAt: null,
        parentThreadId: null,
        parentThreadTitle: null,
        state: 'active',
      },
    });

    expect(threadMap.value.get('t')?.meta.section).toBe('archived');
    expect(threadMap.value.get('t')?.meta.codingAgentChangeState.kind).toBe('none');

    resolveApi({ archived: ['t'] });
    await pending;
  });

  it('protects cascade descendants from stale aggregates too', async () => {
    let resolveApi: (v: { archived: string[] }) => void = () => {};
    (archiveThread as ReturnType<typeof vi.fn>).mockImplementationOnce(
      () => new Promise<{ archived: string[] }>(r => { resolveApi = r; }),
    );

    const map = new Map<string, ThreadState>();
    map.set('parent', makeThreadState('parent', {
      meta: { id: 'parent', title: 'Parent', channel: 'claude_code', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:02Z', status: 'idle', codingAgentChangeState: { kind: 'proposed', requires_restart: false }, codingAgentIsExternalRepo: false, lastRevivedAt: '', messageCount: 1, section: 'inbox', activeChildrenCount: 0 },
    }));
    map.set('child', makeThreadState('child', {
      meta: { id: 'child', title: 'Child', channel: 'claude_code', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:01Z', status: 'idle', codingAgentChangeState: { kind: 'proposed', requires_restart: false }, codingAgentIsExternalRepo: false, lastRevivedAt: '', messageCount: 1, section: 'inbox', activeChildrenCount: 0, parentThreadId: 'parent' },
    }));
    threadMap.value = map;
    focusThread('parent');

    const pending = handleArchiveThread('parent');
    expect(threadMap.value.get('parent')?.meta.section).toBe('archived');
    expect(threadMap.value.get('child')?.meta.section).toBe('archived');

    handleThreadEvent({
      thread_id: 'child',
      seq: 201,
      created: '2026-01-01T00:00:03Z',
      event: { type: 'CodingAgentIdled' },
      aggregate: {
        threadId: 'child',
        title: 'Child',
        channel: 'claude_code',
        initiator: 'user',
        createdAt: '',
        lastActivity: '2026-01-01T00:00:03Z',
        messageCount: 1,
        section: 'inbox',
        status: 'idle',
        activeChildrenCount: 0,
        totalChildrenCount: 0,
        blockingDescendantCount: 0, attentionDescendantCount: 0,
        codingAgentChangeState: { kind: 'proposed', requires_restart: false },
        codingAgentIsExternalRepo: false,
        isSaved: false,
        hasResponse: true,
        lastRevivedAt: null,
        parentThreadId: 'parent',
        parentThreadTitle: 'Parent',
        state: 'active',
      },
    });

    expect(threadMap.value.get('child')?.meta.section).toBe('archived');
    expect(threadMap.value.get('child')?.meta.codingAgentChangeState.kind).toBe('none');

    resolveApi({ archived: ['parent', 'child'] });
    await pending;
  });

  it('lets a post-flip aggregate through once the archive is acknowledged', async () => {
    (archiveThread as ReturnType<typeof vi.fn>).mockResolvedValueOnce({ archived: ['t'] });

    const map = new Map<string, ThreadState>();
    map.set('t', makeThreadState('t', {
      meta: { id: 't', title: 'Thread', channel: 'chat', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:01Z', status: 'idle', codingAgentChangeState: { kind: 'none' }, codingAgentIsExternalRepo: false, lastRevivedAt: '', messageCount: 1, section: 'inbox', activeChildrenCount: 0 },
    }));
    threadMap.value = map;
    focusThread('t');

    await handleArchiveThread('t');
    expect(archiveRequestInFlight('t')).toBe(false);

    handleThreadEvent({
      thread_id: 't',
      seq: 102,
      created: '2026-01-01T00:00:05Z',
      event: { type: 'MessageReceived', text: 'hello', mode: 'chat', event_id: 'm1' },
      aggregate: {
        threadId: 't',
        title: 'Thread',
        channel: 'chat',
        initiator: 'user',
        createdAt: '',
        lastActivity: '2026-01-01T00:00:05Z',
        messageCount: 2,
        section: 'inbox',
        status: 'running',
        activeChildrenCount: 0,
        totalChildrenCount: 0,
        blockingDescendantCount: 0, attentionDescendantCount: 0,
        codingAgentChangeState: { kind: 'none' },
        codingAgentIsExternalRepo: false,
        isSaved: false,
        hasResponse: false,
        lastRevivedAt: null,
        parentThreadId: null,
        parentThreadTitle: null,
        state: 'active',
        summaryVersion: 1,
      },
    });

    expect(threadMap.value.get('t')?.meta.section).toBe('inbox');
  });
});



describe('handleArchiveThread — 409 error toasts', () => {
  beforeEach(() => {
    toasts.value = [];
  });

  function seedReviewThread(): void {
    const map = new Map<string, ThreadState>();
    map.set('parent', makeThreadState('parent', {
      meta: { id: 'parent', title: 'Parent', channel: 'claude_code', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:01Z', status: 'waiting', codingAgentChangeState: { kind: 'proposed', requires_restart: false }, codingAgentIsExternalRepo: false, lastRevivedAt: '', messageCount: 1, section: 'inbox', activeChildrenCount: 0 },
    }));
    threadMap.value = map;
    focusThread('parent');
  }

  async function archiveAndGetToast(err: unknown): Promise<string | undefined> {
    (archiveThread as ReturnType<typeof vi.fn>).mockRejectedValueOnce(err);
    await handleArchiveThread('parent');
    return toasts.value.find(t => t.type === 'error')?.message;
  }

  it("words a sub-thread blocker in the menu's words, and offers to show it", async () => {
    const { ApiError } = await import('../../api/client');
    seedReviewThread();
    const message = await archiveAndGetToast(new ApiError(409, 'descendants_blocking', {
      reason: 'descendants_blocking',
      blocker: 'descendant_running',
      blocking: [{ thread_id: 'child', status: 'running', has_pending_changes: false }],
    }));
    expect(message).toBe('A sub-thread is still running.');
    const toast = toasts.value.find(t => t.type === 'error');
    expect(toast?.title).toBe("Can't archive yet");
    expect(toast?.action?.label).toBe('Show sub-thread');
  });

  it("words the thread's own blocker without a sub-thread to show", async () => {
    const { ApiError } = await import('../../api/client');
    seedReviewThread();
    const message = await archiveAndGetToast(new ApiError(409, 'parent_not_archivable', {
      reason: 'parent_not_archivable',
      blocker: 'running',
      parent_status: 'running',
      has_pending_changes: false,
    }));
    expect(message).toBe('This thread is still running. Stop it first.');
    expect(toasts.value.find(t => t.type === 'error')?.action).toBeUndefined();
  });

  it('archiving an already-archived thread is idempotent success, not a 409 toast', async () => {
    (archiveThread as ReturnType<typeof vi.fn>).mockResolvedValueOnce({ archived: [] });
    seedReviewThread();
    await handleArchiveThread('parent');
    expect(threadMap.value.get('parent')?.meta.section).toBe('archived');
    expect(toasts.value.find(t => t.type === 'error')).toBeUndefined();
  });

  it('a target a Discard holds fails with the engine\'s own reason, not a quiet success', async () => {
    const { ApiError } = await import('../../api/client');
    seedReviewThread();
    const reason = 'A discard is already in progress for this thread. Try again once it has finished';
    const message = await archiveAndGetToast(new ApiError(409, reason, {
      reason: 'discard_in_progress',
      message: reason,
    }));
    expect(message).toBe(`Can't archive yet: ${reason}`);
    expect(threadMap.value.get('parent')?.meta.section).toBe('inbox');
  });

  it('names every member the archive skipped, puts it back, and says to retry', async () => {
    const map = new Map<string, ThreadState>();
    map.set('parent', makeThreadState('parent', {
      meta: { id: 'parent', title: 'Parent', channel: 'claude_code', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:01Z', status: 'idle', codingAgentChangeState: { kind: 'none' }, codingAgentIsExternalRepo: false, lastRevivedAt: '', messageCount: 1, section: 'inbox', activeChildrenCount: 0 },
    }));
    map.set('child', makeThreadState('child', {
      meta: { id: 'child', title: 'Fix the login page', channel: 'claude_code', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:00Z', status: 'idle', codingAgentChangeState: { kind: 'none' }, codingAgentIsExternalRepo: false, lastRevivedAt: '', messageCount: 1, section: 'inbox', activeChildrenCount: 0, parentThreadId: 'parent' },
    }));
    threadMap.value = map;
    focusThread('parent');
    const reason = 'An apply is already in progress for this thread. It finishes on its own';
    (archiveThread as ReturnType<typeof vi.fn>).mockResolvedValueOnce({
      archived: ['parent'],
      skipped: [{ thread_id: 'child', reason: 'apply_in_progress', message: reason }],
    });

    await handleArchiveThread('parent');

    expect(threadMap.value.get('parent')?.meta.section).toBe('archived');
    expect(threadMap.value.get('child')?.meta.section).toBe('inbox');
    const toast = toasts.value.find(t => t.type === 'warning');
    expect(toast?.message).toContain('Fix the login page');
    expect(toast?.message).toContain(reason);
    expect(toast?.message).toMatch(/archive again/i);
  });

  it('gives the target its focus back when the archive skipped it', async () => {
    const map = new Map<string, ThreadState>();
    for (const [id, updatedAt] of [['parent', '2026-01-01T00:00:02Z'], ['other', '2026-01-01T00:00:01Z']]) {
      map.set(id, makeThreadState(id, {
        meta: { id, title: id, channel: 'claude_code', saved: false, createdAt: '', updatedAt, status: 'waiting', codingAgentChangeState: { kind: 'proposed', requires_restart: false }, codingAgentIsExternalRepo: false, lastRevivedAt: '', messageCount: 1, section: 'inbox', activeChildrenCount: 0 },
      }));
    }
    threadMap.value = map;
    focusThread('parent');
    (archiveThread as ReturnType<typeof vi.fn>).mockResolvedValueOnce({
      archived: [],
      skipped: [{ thread_id: 'parent', reason: 'not_archivable', message: 'It changed state after the archive began, so it was left open' }],
    });

    await handleArchiveThread('parent');

    expect(threadMap.value.get('parent')?.meta.section).toBe('inbox');
    expect(focusedThreadId.value).toBe('parent');
  });

  it('an archive response from an older engine, with no skipped list, archives quietly', async () => {
    (archiveThread as ReturnType<typeof vi.fn>).mockResolvedValueOnce({ archived: ['parent'] });
    seedReviewThread();
    await handleArchiveThread('parent');
    expect(threadMap.value.get('parent')?.meta.section).toBe('archived');
    expect(toasts.value.find(t => t.type === 'warning' || t.type === 'error')).toBeUndefined();
  });

  it('formats parent_has_pending_changes with the Apply/Discard hint', async () => {
    const { ApiError } = await import('../../api/client');
    seedReviewThread();
    const message = await archiveAndGetToast(new ApiError(409, 'parent_has_pending_changes', {
      reason: 'parent_has_pending_changes',
      blocker: 'pending_change',
    }));
    expect(message).toBe('Apply or discard the pending change first.');
  });

  it('falls back to the generic message for non-ApiError failures', async () => {
    seedReviewThread();
    const message = await archiveAndGetToast(new Error('boom'));
    expect(message).toBe('Failed to archive thread: boom');
  });

  it('falls back to the generic message for ApiError without a structured body', async () => {
    const { ApiError } = await import('../../api/client');
    seedReviewThread();
    const message = await archiveAndGetToast(new ApiError(500, 'Internal Server Error'));
    expect(message).toBe('Failed to archive thread: 500 Internal Server Error');
  });
});


describe('handleArchiveThread — unsent draft confirm', () => {
  beforeEach(() => {
    toasts.value = [];
    confirmState.value = { visible: false, message: '', okLabel: 'Delete' };
    (putComposeOnThread as ReturnType<typeof vi.fn>).mockClear().mockResolvedValue({ status: 'applied', report: null });
  });

  function seedDraftThread(): void {
    const map = new Map<string, ThreadState>();
    map.set('t', makeThreadState('t', {
      meta: { id: 't', title: 'Drafted', channel: 'chat', status: 'idle', messageCount: 1, section: 'inbox', composeText: 'half-written reply' },
    }));
    threadMap.value = map;
    focusThread('t');
  }

  function clickDiscard(): void {
    confirmState.value.extraAction?.onClick();
    confirmState.value.resolve?.(false);
  }

  it('archives directly, with no confirm, when the thread has no draft', async () => {
    const map = new Map<string, ThreadState>();
    map.set('t', makeThreadState('t', {
      meta: { id: 't', title: 'No draft', channel: 'chat', status: 'idle', messageCount: 1, section: 'inbox' },
    }));
    threadMap.value = map;
    focusThread('t');
    (archiveThread as ReturnType<typeof vi.fn>).mockClear();

    await handleArchiveThread('t');

    expect(confirmState.value.visible).toBe(false);
    expect(archiveThread).toHaveBeenCalledWith('t');
  });

  it('shows the discard-draft confirm with the right labels when the thread has a draft', async () => {
    seedDraftThread();
    (archiveThread as ReturnType<typeof vi.fn>).mockClear();

    const pending = handleArchiveThread('t');
    expect(confirmState.value.visible).toBe(true);
    expect(confirmState.value.message).toBe('This thread has an unsent draft. Discard it too?');
    expect(confirmState.value.okLabel).toBe('Keep draft');
    expect(confirmState.value.extraAction?.label).toBe('Discard draft');

    confirmState.value.resolve?.(false);
    await pending;
    expect(archiveThread).not.toHaveBeenCalled();
  });

  it('keeps the draft and archives when the user picks "Keep draft"', async () => {
    seedDraftThread();
    (archiveThread as ReturnType<typeof vi.fn>).mockClear();

    const pending = handleArchiveThread('t');
    expect(confirmState.value.visible).toBe(true);
    confirmState.value.resolve?.(true);
    await pending;

    expect(archiveThread).toHaveBeenCalledWith('t');
    expect(getDraft('t').text).toBe('half-written reply');
    expect(draftPresentThreadIds.value.has('t')).toBe(true);
    expect(putComposeOnThread).not.toHaveBeenCalled();
  });

  it('discards the draft and archives when the user picks "Discard draft"', async () => {
    seedDraftThread();
    (archiveThread as ReturnType<typeof vi.fn>).mockClear();

    const pending = handleArchiveThread('t');
    expect(confirmState.value.visible).toBe(true);
    clickDiscard();
    await pending;

    expect(archiveThread).toHaveBeenCalledWith('t');
    expect(getDraft('t').text).toBe('');
    expect(draftPresentThreadIds.value.has('t')).toBe(false);
    expect(putComposeOnThread).toHaveBeenCalledWith('t', '', [], null, undefined, undefined);
  });

  it('aborts the archive entirely on Cancel, leaving the draft intact', async () => {
    seedDraftThread();
    (archiveThread as ReturnType<typeof vi.fn>).mockClear();

    const pending = handleArchiveThread('t');
    expect(confirmState.value.visible).toBe(true);
    confirmState.value.resolve?.(false);
    await pending;

    expect(archiveThread).not.toHaveBeenCalled();
    expect(threadMap.value.get('t')?.meta.section).toBe('inbox');
    expect(getDraft('t').text).toBe('half-written reply');
    expect(putComposeOnThread).not.toHaveBeenCalled();
  });

  it('pluralizes the prompt and clears every family draft on discard', async () => {
    const map = new Map<string, ThreadState>();
    map.set('parent', makeThreadState('parent', {
      meta: { id: 'parent', title: 'Parent', channel: 'chat', status: 'idle', messageCount: 1, section: 'inbox', composeText: 'parent draft' },
    }));
    map.set('child', makeThreadState('child', {
      meta: { id: 'child', title: 'Child', channel: 'chat', status: 'idle', messageCount: 1, section: 'inbox', parentThreadId: 'parent', composeText: 'child draft' },
    }));
    threadMap.value = map;
    focusThread('parent');
    (archiveThread as ReturnType<typeof vi.fn>).mockClear();

    const pending = handleArchiveThread('parent');
    expect(confirmState.value.visible).toBe(true);
    expect(confirmState.value.message).toBe('These threads have unsent drafts. Discard them too?');
    expect(confirmState.value.okLabel).toBe('Keep drafts');
    expect(confirmState.value.extraAction?.label).toBe('Discard drafts');
    clickDiscard();
    await pending;

    expect(getDraft('parent').text).toBe('');
    expect(getDraft('child').text).toBe('');
    expect(draftPresentThreadIds.value.has('parent')).toBe(false);
    expect(draftPresentThreadIds.value.has('child')).toBe(false);
    expect(putComposeOnThread).toHaveBeenCalledWith('parent', '', [], null, undefined, undefined);
    expect(putComposeOnThread).toHaveBeenCalledWith('child', '', [], null, undefined, undefined);
  });

  it('preserves the draft when the archive fails after "Discard draft" was chosen', async () => {
    seedDraftThread();
    (archiveThread as ReturnType<typeof vi.fn>).mockClear().mockRejectedValueOnce(new Error('boom'));

    const pending = handleArchiveThread('t');
    clickDiscard();
    await pending;

    expect(threadMap.value.get('t')?.meta.section).toBe('inbox');
    expect(getDraft('t').text).toBe('half-written reply');
    expect(draftPresentThreadIds.value.has('t')).toBe(true);
    expect(putComposeOnThread).not.toHaveBeenCalled();
  });

  it('restores the draft locally when the compose PUT fails after discard', async () => {
    seedDraftThread();
    (archiveThread as ReturnType<typeof vi.fn>).mockClear();
    (putComposeOnThread as ReturnType<typeof vi.fn>).mockClear().mockRejectedValueOnce(new Error('net'));

    const pending = handleArchiveThread('t');
    clickDiscard();
    await pending;
    await Promise.resolve();
    await Promise.resolve();

    expect(archiveThread).toHaveBeenCalledWith('t');
    expect(getDraft('t').text).toBe('half-written reply');
    expect(draftPresentThreadIds.value.has('t')).toBe(true);
    expect(toasts.value.some(t => t.type === 'error')).toBe(true);
  });
});


describe('handleArchiveThread: live subscription confirm', () => {
  beforeEach(() => {
    toasts.value = [];
    confirmState.value = { visible: false, message: '', okLabel: 'Delete' };
  });

  const wait = (id: string, reason: string, eventType: string) => ({
    wait_id: id,
    on: [{ event_type: eventType }],
    reason,
    expires_at: new Date(Date.now() + 3_600_000).toISOString(),
  });

  function detailLines(): string[] {
    const d = confirmState.value.details;
    if (!d) return [];
    return d.groups.flatMap((g) => [g.header, ...g.items]);
  }

  it('archives on one tap when the cascade holds no subscriptions', async () => {
    const map = new Map<string, ThreadState>();
    map.set('t', makeThreadState('t', {
      meta: { id: 't', title: 'Quiet', channel: 'chat', status: 'idle', messageCount: 1, section: 'inbox' },
    }));
    threadMap.value = map;
    focusThread('t');
    (archiveThread as ReturnType<typeof vi.fn>).mockClear();

    await handleArchiveThread('t');

    expect(confirmState.value.visible).toBe(false);
    expect(archiveThread).toHaveBeenCalledWith('t');
  });

  it('names what each subscription is watching, rather than printing a count', async () => {
    const map = new Map<string, ThreadState>();
    map.set('t', makeThreadState('t', {
      meta: {
        id: 't', title: 'Watching', channel: 'chat', status: 'idle', messageCount: 1, section: 'inbox',
        liveEventWaitCount: 2,
        liveEventWaits: [
          wait('w1', 'waiting for the release build', 'ChangeProposed'),
          wait('w2', 'waiting for the nightly import', 'OuraSleepImported'),
        ],
      },
    }));
    threadMap.value = map;
    focusThread('t');
    (archiveThread as ReturnType<typeof vi.fn>).mockClear();

    const pending = handleArchiveThread('t');
    expect(confirmState.value.visible).toBe(true);
    expect(confirmState.value.message).toBe('Archiving stops waiting for 2 events. They will not fire.');
    expect(confirmState.value.okLabel).toBe('Archive');
    expect(confirmState.value.cancelLabel).toBe('Cancel');
    expect(detailLines()).toEqual([
      'This thread',
      'waiting for the release build (change proposed)',
      'waiting for the nightly import (oura sleep imported)',
    ]);

    confirmState.value.resolve?.(false);
    await pending;
  });

  it('Cancel aborts the archive and leaves the thread where it was', async () => {
    const map = new Map<string, ThreadState>();
    map.set('t', makeThreadState('t', {
      meta: {
        id: 't', title: 'Watching', channel: 'chat', status: 'idle', messageCount: 1, section: 'inbox',
        liveEventWaitCount: 1,
        liveEventWaits: [wait('w1', 'waiting for the release build', 'ChangeProposed')],
      },
    }));
    threadMap.value = map;
    focusThread('t');
    (archiveThread as ReturnType<typeof vi.fn>).mockClear();

    const pending = handleArchiveThread('t');
    expect(confirmState.value.message).toBe(
      'Archiving stops what this thread is waiting for. It will not fire.',
    );
    confirmState.value.resolve?.(false);
    await pending;

    expect(archiveThread).not.toHaveBeenCalled();
    expect(threadMap.value.get('t')!.meta.section).toBe('inbox');
  });

  it('Archive proceeds, and the archive still cancels the subscriptions', async () => {
    const map = new Map<string, ThreadState>();
    map.set('t', makeThreadState('t', {
      meta: {
        id: 't', title: 'Watching', channel: 'chat', status: 'idle', messageCount: 1, section: 'inbox',
        liveEventWaitCount: 1,
        liveEventWaits: [wait('w1', 'waiting for the release build', 'ChangeProposed')],
      },
    }));
    threadMap.value = map;
    focusThread('t');
    (archiveThread as ReturnType<typeof vi.fn>).mockClear();

    const pending = handleArchiveThread('t');
    confirmState.value.resolve?.(true);
    await pending;

    expect(archiveThread).toHaveBeenCalledWith('t');
  });

  it('walks the whole cascade, so a sub-thread parked on a long watch is counted', async () => {
    const map = new Map<string, ThreadState>();
    map.set('parent', makeThreadState('parent', {
      meta: { id: 'parent', title: 'Parent', channel: 'chat', status: 'idle', messageCount: 1, section: 'inbox' },
    }));
    map.set('child', makeThreadState('child', {
      meta: { id: 'child', title: 'Child', channel: 'chat', status: 'idle', messageCount: 1, section: 'inbox', parentThreadId: 'parent' },
    }));
    map.set('grandchild', makeThreadState('grandchild', {
      meta: {
        id: 'grandchild', title: 'Grandchild', channel: 'chat', status: 'idle', messageCount: 1,
        section: 'inbox', parentThreadId: 'child',
        liveEventWaitCount: 1,
        liveEventWaits: [wait('w1', 'waiting for the deploy to land', 'ReleasePublished')],
      },
    }));
    threadMap.value = map;
    focusThread('parent');
    (archiveThread as ReturnType<typeof vi.fn>).mockClear();

    const pending = handleArchiveThread('parent');
    expect(confirmState.value.visible).toBe(true);
    expect(detailLines()).toEqual([
      'Grandchild',
      'waiting for the deploy to land (release published)',
    ]);

    confirmState.value.resolve?.(false);
    await pending;
  });

  it('counts what it cannot name, for a sub-thread whose events were never loaded', async () => {
    const map = new Map<string, ThreadState>();
    map.set('parent', makeThreadState('parent', {
      meta: {
        id: 'parent', title: 'Parent', channel: 'chat', status: 'idle', messageCount: 1, section: 'inbox',
        liveEventWaitCount: 1,
        liveEventWaits: [wait('w1', 'waiting for the release build', 'ChangeProposed')],
      },
    }));
    map.set('child', makeThreadState('child', {
      meta: {
        id: 'child', title: 'Unopened', channel: 'chat', status: 'idle', messageCount: 1,
        section: 'inbox', parentThreadId: 'parent',
        liveEventWaitCount: 2,
        liveEventWaits: [],
      },
    }));
    threadMap.value = map;
    focusThread('parent');

    const pending = handleArchiveThread('parent');
    expect(confirmState.value.message).toBe('Archiving stops waiting for 3 events. They will not fire.');
    expect(detailLines()).toEqual([
      'This thread',
      'waiting for the release build (change proposed)',
      '2 more on sub-threads',
    ]);

    confirmState.value.resolve?.(false);
    await pending;
  });

  it('asks about the draft first, then about the subscriptions', async () => {
    const map = new Map<string, ThreadState>();
    map.set('t', makeThreadState('t', {
      meta: {
        id: 't', title: 'Both', channel: 'chat', status: 'idle', messageCount: 1, section: 'inbox',
        composeText: 'half-written reply',
        liveEventWaitCount: 1,
        liveEventWaits: [wait('w1', 'waiting for the release build', 'ChangeProposed')],
      },
    }));
    threadMap.value = map;
    focusThread('t');
    (archiveThread as ReturnType<typeof vi.fn>).mockClear();

    const pending = handleArchiveThread('t');
    expect(confirmState.value.message).toBe('This thread has an unsent draft. Discard it too?');
    confirmState.value.resolve?.(true);
    await Promise.resolve();
    await Promise.resolve();
    expect(confirmState.value.message).toBe(
      'Archiving stops what this thread is waiting for. It will not fire.',
    );
    confirmState.value.resolve?.(true);
    await pending;

    expect(archiveThread).toHaveBeenCalledWith('t');
  });
});


describe('subscriptionsStoppedByArchive', () => {
  const wait2 = (id: string, reason: string, eventType: string) => ({
    wait_id: id,
    on: [{ event_type: eventType }],
    reason,
    expires_at: new Date(Date.now() + 3_600_000).toISOString(),
  });

  it('counts the shortfall even when the thread named SOME of its subscriptions', () => {
    const map = new Map<string, ThreadState>();
    map.set('t', makeThreadState('t', {
      meta: {
        id: 't', title: 'Partly loaded', channel: 'chat', status: 'idle', messageCount: 1,
        section: 'inbox',
        liveEventWaitCount: 3,
        liveEventWaits: [wait2('w1', 'waiting for the release build', 'ChangeProposed')],
      },
    }));
    threadMap.value = map;

    const stopping = subscriptionsStoppedByArchive(new Set(['t']), 't')!;
    expect(stopping.message).toBe('Archiving stops waiting for 3 events. They will not fire.');
    expect(stopping.details.groups.map((g) => g.header)).toEqual([
      'This thread',
      '2 more on sub-threads',
    ]);
  });

  it('returns null when nothing in the cascade is watching', () => {
    const map = new Map<string, ThreadState>();
    map.set('t', makeThreadState('t', {
      meta: { id: 't', title: 'Quiet', channel: 'chat', status: 'idle', messageCount: 1, section: 'inbox' },
    }));
    threadMap.value = map;
    expect(subscriptionsStoppedByArchive(new Set(['t']), 't')).toBeNull();
  });

  it('skips a cascade member that is no longer in the map', () => {
    threadMap.value = new Map();
    expect(subscriptionsStoppedByArchive(new Set(['gone']), 'gone')).toBeNull();
  });
});
