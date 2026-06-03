import { describe, it, expect, beforeEach, vi } from 'vitest';

// Polyfill localStorage before store.ts is imported at module level.
// vi.hoisted runs before any imports are resolved.
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
  // scrollToBottom() needs document.querySelector and requestAnimationFrame
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
import { focusPromptNow } from '../../components/chat/promptFocus';
import { drawerOpen } from '../../components/layout/Drawer';
import { _resetComposeDraftsForTesting } from '../composeDrafts';
import { archivingThreadIds, focusedThreadId, generatedTitleIds, getReviewThreads, mobileView, resetCCPendingPreferences, threadDrawerOpen, threadMap, toasts } from '../store';
import { upsertThread } from './thread-loading';
import { handleThreadEvent } from './thread-sync';
import { focusThread, handleArchiveThread } from './threads';

// Mock the API module
vi.mock('../../api/threads', () => ({
  fetchThreads: vi.fn(),
  fetchThreadEvents: vi.fn().mockResolvedValue({ events: [], currentAggregate: null }),
  fetchThreadMessages: vi.fn(),
  saveThread: vi.fn().mockResolvedValue(undefined),
  archiveThread: vi.fn().mockResolvedValue({ archived: [] }),
}));

// Use the real isComposeFocusedHere — the compose-preservation tests below
// plant a fake activeElement + querySelectorAll to drive its branches. Mocking
// only the focus-side-effect helpers keeps the predicate honest if it grows.
vi.mock('../../components/chat/promptFocus', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../components/chat/promptFocus')>()),
  focusPromptNow: vi.fn(),
  focusIfNeeded: vi.fn(),
  composeHandlers: vi.fn(),
}));

vi.mock('../../api/client', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../api/client')>()),
  API_BASE: '',
  submitChat: vi.fn().mockResolvedValue(undefined),
  cancelChat: vi.fn(),
  stopClaudeCode: vi.fn(),
  putComposeOnThread: vi.fn().mockResolvedValue(undefined),
  ensureThreadStarted: vi.fn().mockResolvedValue(undefined),
  deleteThread: vi.fn().mockResolvedValue(undefined),
}));

// ---------------------------------------------------------------------------
// Setup
// ---------------------------------------------------------------------------

beforeEach(() => {
  threadMap.value = new Map();
  _resetComposeDraftsForTesting();
  focusedThreadId.value = null;
  mobileView.value = 'thread';
  threadDrawerOpen.value = false;
  drawerOpen.value = false;
  resetCCPendingPreferences();
  generatedTitleIds.clear();
  archivingThreadIds.value = new Set();
  localStorage.removeItem('lucidos-focused-thread');
});

describe('handleArchiveThread', () => {
  it('focuses the next review thread after dismissing', async () => {
    // Set up: t1 (focused, waiting/inbox = review), t2 (also waiting/inbox = review)
    const map = new Map<string, ThreadState>();
    map.set('t1', makeThreadState('t1', {
      meta: { id: 't1', title: 'Thread t1', channel: 'claude_code', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:01Z', status: 'waiting', codingAgentProposed: true, codingAgentRequiresRestart: false, codingAgentIsExternalRepo: false, codingAgentApplying: false, lastRevivedAt: '', messageCount: 1, section: 'inbox', activeChildrenCount: 0 },
    }));
    map.set('t2', makeThreadState('t2', {
      meta: { id: 't2', title: 'Thread t2', channel: 'claude_code', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:00Z', status: 'waiting', codingAgentProposed: true, codingAgentRequiresRestart: false, codingAgentIsExternalRepo: false, codingAgentApplying: false, lastRevivedAt: '', messageCount: 1, section: 'inbox', activeChildrenCount: 0 },
    }));
    threadMap.value = map;
    focusThread('t1');

    await handleArchiveThread('t1');

    // Should focus t2 (next review thread), not unfocus
    expect(focusedThreadId.value).toBe('t2');
  });

  it('unfocuses and resets mobileView when no more review threads remain', async () => {
    const map = new Map<string, ThreadState>();
    map.set('t1', makeThreadState('t1', {
      meta: { id: 't1', title: 'Thread t1', channel: 'claude_code', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:00Z', status: 'waiting', codingAgentProposed: true, codingAgentRequiresRestart: false, codingAgentIsExternalRepo: false, codingAgentApplying: false, lastRevivedAt: '', messageCount: 1, section: 'inbox', activeChildrenCount: 0 },
    }));
    threadMap.value = map;
    focusThread('t1');
    mobileView.value = 'threads';

    await handleArchiveThread('t1');

    expect(focusedThreadId.value).toBeNull();
    expect(mobileView.value).toBe('thread');
  });

  it('does not focus prompt when dismissing the last review thread', async () => {
    // Bug: dismissing the last review thread called focusPromptNow(), which
    // opened the keyboard on mobile. The compose view should appear with the
    // prompt unfocused and the header visible.
    const map = new Map<string, ThreadState>();
    map.set('t1', makeThreadState('t1', {
      meta: { id: 't1', title: 'Last review', channel: 'claude_code', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:00Z', status: 'waiting', codingAgentProposed: true, codingAgentRequiresRestart: false, codingAgentIsExternalRepo: false, codingAgentApplying: false, lastRevivedAt: '', messageCount: 1, section: 'inbox', activeChildrenCount: 0 },
    }));
    threadMap.value = map;
    focusThread('t1');
    (focusPromptNow as ReturnType<typeof vi.fn>).mockClear();

    await handleArchiveThread('t1');

    expect(focusPromptNow).not.toHaveBeenCalled();
    expect(focusedThreadId.value).toBeNull();
  });

  it('focuses the next thread below, not the top one', async () => {
    // 3 review threads sorted by updatedAt desc: t1 (newest), t2 (middle), t3 (oldest)
    // Dismissing t2 should focus t3 (below), not t1 (top)
    const map = new Map<string, ThreadState>();
    map.set('t1', makeThreadState('t1', {
      meta: { id: 't1', title: 'Thread t1', channel: 'claude_code', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:02Z', status: 'waiting', codingAgentProposed: false, codingAgentRequiresRestart: false, codingAgentIsExternalRepo: false, codingAgentApplying: false, lastRevivedAt: '', messageCount: 1, section: 'inbox', activeChildrenCount: 0 },
    }));
    map.set('t2', makeThreadState('t2', {
      meta: { id: 't2', title: 'Thread t2', channel: 'claude_code', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:01Z', status: 'waiting', codingAgentProposed: false, codingAgentRequiresRestart: false, codingAgentIsExternalRepo: false, codingAgentApplying: false, lastRevivedAt: '', messageCount: 1, section: 'inbox', activeChildrenCount: 0 },
    }));
    map.set('t3', makeThreadState('t3', {
      meta: { id: 't3', title: 'Thread t3', channel: 'claude_code', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:00Z', status: 'waiting', codingAgentProposed: false, codingAgentRequiresRestart: false, codingAgentIsExternalRepo: false, codingAgentApplying: false, lastRevivedAt: '', messageCount: 1, section: 'inbox', activeChildrenCount: 0 },
    }));
    threadMap.value = map;
    focusThread('t2');

    await handleArchiveThread('t2');

    expect(focusedThreadId.value).toBe('t3');
  });

  it('navigates to next review thread after apply+archive (regression: Apply used to skip Archive)', async () => {
    // Simulates the fixed flow: after Apply, thread stays in review (section=inbox,
    // codingAgentProposed=false), Archive button appears, clicking Archive navigates to next thread.
    // Previously Apply moved the thread to ARCHIVE immediately, skipping Archive button.
    const map = new Map<string, ThreadState>();
    // t1: just applied — still in review with Archive button (inbox, no changes, idle)
    map.set('t1', makeThreadState('t1', {
      meta: { id: 't1', title: 'Applied thread', channel: 'claude_code', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:01Z', status: 'idle', codingAgentProposed: false, codingAgentRequiresRestart: false, codingAgentIsExternalRepo: false, codingAgentApplying: false, lastRevivedAt: '', messageCount: 1, section: 'inbox', activeChildrenCount: 0 },
    }));
    // t2: another review thread waiting for attention
    map.set('t2', makeThreadState('t2', {
      meta: { id: 't2', title: 'Pending thread', channel: 'claude_code', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:00Z', status: 'waiting', codingAgentProposed: true, codingAgentRequiresRestart: false, codingAgentIsExternalRepo: false, codingAgentApplying: false, lastRevivedAt: '', messageCount: 1, section: 'inbox', activeChildrenCount: 0 },
    }));
    threadMap.value = map;
    focusThread('t1');

    await handleArchiveThread('t1');

    // After Done on t1, should navigate to t2 (next review thread)
    expect(focusedThreadId.value).toBe('t2');
  });

  it('keeps thread drawer open on desktop when all reviews are done (compose view)', async () => {
    // Bug: dismissing the last review on desktop closed the thread drawer
    // because handleArchiveThread called navigateToPane('thread'), which
    // unconditionally cleared threadDrawerOpen. The drawer must remain open
    // so the user can pick another thread from the compose view.
    const map = new Map<string, ThreadState>();
    map.set('t1', makeThreadState('t1', {
      meta: { id: 't1', title: 'Last review', channel: 'claude_code', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:00Z', status: 'waiting', codingAgentProposed: true, codingAgentRequiresRestart: false, codingAgentIsExternalRepo: false, codingAgentApplying: false, lastRevivedAt: '', messageCount: 1, section: 'inbox', activeChildrenCount: 0 },
    }));
    threadMap.value = map;
    focusThread('t1');
    threadDrawerOpen.value = true;

    await handleArchiveThread('t1');

    expect(focusedThreadId.value).toBeNull();
    expect(threadDrawerOpen.value).toBe(true);
  });

  it('focuses the thread above when dismissing the last item', async () => {
    // 2 review threads: t1 (top), t2 (bottom). Dismissing t2 should focus t1.
    const map = new Map<string, ThreadState>();
    map.set('t1', makeThreadState('t1', {
      meta: { id: 't1', title: 'Thread t1', channel: 'claude_code', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:01Z', status: 'waiting', codingAgentProposed: false, codingAgentRequiresRestart: false, codingAgentIsExternalRepo: false, codingAgentApplying: false, lastRevivedAt: '', messageCount: 1, section: 'inbox', activeChildrenCount: 0 },
    }));
    map.set('t2', makeThreadState('t2', {
      meta: { id: 't2', title: 'Thread t2', channel: 'claude_code', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:00Z', status: 'waiting', codingAgentProposed: false, codingAgentRequiresRestart: false, codingAgentIsExternalRepo: false, codingAgentApplying: false, lastRevivedAt: '', messageCount: 1, section: 'inbox', activeChildrenCount: 0 },
    }));
    threadMap.value = map;
    focusThread('t2');

    await handleArchiveThread('t2');

    expect(focusedThreadId.value).toBe('t1');
  });

  it('skips cascaded descendants and lands on the next non-archived review thread', async () => {
    // Archiving a parent cascades to its descendants. The frontend computes
    // the cascade locally by walking parentThreadId so it can drop descendants
    // out of review immediately, without waiting for the API response.
    //
    // Review order (tier 1 — all have codingAgentProposed=true, sorted by
    // recency desc): parent, child, sibling.
    const map = new Map<string, ThreadState>();
    map.set('parent', makeThreadState('parent', {
      meta: { id: 'parent', title: 'Parent', channel: 'claude_code', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:02Z', status: 'waiting', codingAgentProposed: true, codingAgentRequiresRestart: false, codingAgentIsExternalRepo: false, codingAgentApplying: false, lastRevivedAt: '', messageCount: 1, section: 'inbox', activeChildrenCount: 0 },
    }));
    map.set('child', makeThreadState('child', {
      meta: { id: 'child', title: 'Child', channel: 'claude_code', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:01Z', status: 'waiting', codingAgentProposed: true, codingAgentRequiresRestart: false, codingAgentIsExternalRepo: false, codingAgentApplying: false, lastRevivedAt: '', messageCount: 1, section: 'inbox', activeChildrenCount: 0, parentThreadId: 'parent' },
    }));
    map.set('sibling', makeThreadState('sibling', {
      meta: { id: 'sibling', title: 'Sibling', channel: 'claude_code', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:00Z', status: 'waiting', codingAgentProposed: true, codingAgentRequiresRestart: false, codingAgentIsExternalRepo: false, codingAgentApplying: false, lastRevivedAt: '', messageCount: 1, section: 'inbox', activeChildrenCount: 0 },
    }));
    threadMap.value = map;
    focusThread('parent');

    await handleArchiveThread('parent');

    // Must land on the unrelated sibling — not on the cascade-archived child.
    expect(focusedThreadId.value).toBe('sibling');
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
      meta: { updatedAt: '2026-01-01T00:00:03Z', channel: 'claude_code', status: 'waiting', codingAgentProposed: true, messageCount: 1, section: 'inbox' },
    }));
    map.set('draft', makeThreadState('draft', {
      meta: { updatedAt: '2026-01-01T00:00:02Z', channel: 'claude_code', section: 'inbox', state: 'composing' },
    }));
    map.set('sibling', makeThreadState('sibling', {
      meta: { updatedAt: '2026-01-01T00:00:01Z', channel: 'claude_code', status: 'waiting', codingAgentProposed: true, messageCount: 1, section: 'inbox' },
    }));
    threadMap.value = map;
    focusThread('focused');

    await handleArchiveThread('focused');

    expect(focusedThreadId.value).toBe('sibling');
  });

  it('cascading archive still lands on a real review sibling, not a discarded orphan', async () => {
    const map = new Map<string, ThreadState>();
    map.set('parent', makeThreadState('parent', {
      meta: { updatedAt: '2026-01-01T00:00:04Z', channel: 'claude_code', status: 'waiting', codingAgentProposed: true, messageCount: 1, section: 'inbox' },
    }));
    map.set('child', makeThreadState('child', {
      meta: { updatedAt: '2026-01-01T00:00:03Z', channel: 'claude_code', status: 'waiting', codingAgentProposed: true, messageCount: 1, section: 'inbox', parentThreadId: 'parent' },
    }));
    map.set('orphan', makeThreadState('orphan', {
      meta: { updatedAt: '2026-01-01T00:00:02Z', channel: 'claude_code', section: 'inbox', state: 'discarded' },
    }));
    map.set('sibling', makeThreadState('sibling', {
      meta: { updatedAt: '2026-01-01T00:00:01Z', channel: 'claude_code', status: 'waiting', codingAgentProposed: true, messageCount: 1, section: 'inbox' },
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

// ---------------------------------------------------------------------------
// handleArchiveThread — optimistic UI
// ---------------------------------------------------------------------------
// The row + every descendant must drop out of review synchronously, before
// the archive API resolves. The user-perceived latency was the await on the
// per-thread cascade emit loop; the SSE round-trip lands later and just
// confirms what we already did.
// ---------------------------------------------------------------------------

describe('handleArchiveThread — optimistic UI', () => {
  it('drops the target out of review before the API resolves', async () => {
    let resolveApi: (v: { archived: string[] }) => void = () => {};
    (archiveThread as ReturnType<typeof vi.fn>).mockImplementationOnce(
      () => new Promise<{ archived: string[] }>(r => { resolveApi = r; }),
    );

    const map = new Map<string, ThreadState>();
    map.set('parent', makeThreadState('parent', {
      meta: { id: 'parent', title: 'Parent', channel: 'claude_code', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:01Z', status: 'waiting', codingAgentProposed: true, codingAgentRequiresRestart: false, codingAgentIsExternalRepo: false, codingAgentApplying: false, lastRevivedAt: '', messageCount: 1, section: 'inbox', activeChildrenCount: 0 },
    }));
    map.set('sibling', makeThreadState('sibling', {
      meta: { id: 'sibling', title: 'Sibling', channel: 'claude_code', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:00Z', status: 'waiting', codingAgentProposed: true, codingAgentRequiresRestart: false, codingAgentIsExternalRepo: false, codingAgentApplying: false, lastRevivedAt: '', messageCount: 1, section: 'inbox', activeChildrenCount: 0 },
    }));
    threadMap.value = map;
    focusThread('parent');

    // Don't await — verify mid-flight (API has not resolved).
    const pending = handleArchiveThread('parent');

    expect(getReviewThreads().some(t => t.meta.id === 'parent')).toBe(false);
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
      meta: { id: 'parent', title: 'Parent', channel: 'claude_code', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:02Z', status: 'waiting', codingAgentProposed: true, codingAgentRequiresRestart: false, codingAgentIsExternalRepo: false, codingAgentApplying: false, lastRevivedAt: '', messageCount: 1, section: 'inbox', activeChildrenCount: 0 },
    }));
    map.set('child', makeThreadState('child', {
      meta: { id: 'child', title: 'Child', channel: 'claude_code', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:01Z', status: 'waiting', codingAgentProposed: true, codingAgentRequiresRestart: false, codingAgentIsExternalRepo: false, codingAgentApplying: false, lastRevivedAt: '', messageCount: 1, section: 'inbox', activeChildrenCount: 0, parentThreadId: 'parent' },
    }));
    map.set('grandchild', makeThreadState('grandchild', {
      meta: { id: 'grandchild', title: 'Grandchild', channel: 'claude_code', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:00Z', status: 'waiting', codingAgentProposed: true, codingAgentRequiresRestart: false, codingAgentIsExternalRepo: false, codingAgentApplying: false, lastRevivedAt: '', messageCount: 1, section: 'inbox', activeChildrenCount: 0, parentThreadId: 'child' },
    }));
    threadMap.value = map;
    focusThread('parent');

    const pending = handleArchiveThread('parent');

    // The whole family must vanish from review before the API resolves.
    const ids = getReviewThreads().map(t => t.meta.id);
    expect(ids).not.toContain('parent');
    expect(ids).not.toContain('child');
    expect(ids).not.toContain('grandchild');

    resolveApi({ archived: ['parent', 'child', 'grandchild'] });
    await pending;
  });

  it('leaves the user where they navigated to if archive rejects mid-flight', async () => {
    // Bug guard: if the user navigates away from `nextId` (e.g. picks a
    // different thread from the drawer) while the archive API is still
    // in flight, the rollback must NOT yank them back to the rejected
    // thread — they made an active choice and we should respect it.
    let rejectApi: (e: Error) => void = () => {};
    (archiveThread as ReturnType<typeof vi.fn>).mockImplementationOnce(
      () => new Promise<{ archived: string[] }>((_resolve, reject) => { rejectApi = reject; }),
    );

    const map = new Map<string, ThreadState>();
    map.set('parent', makeThreadState('parent', {
      meta: { id: 'parent', title: 'Parent', channel: 'claude_code', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:02Z', status: 'waiting', codingAgentProposed: true, codingAgentRequiresRestart: false, codingAgentIsExternalRepo: false, codingAgentApplying: false, lastRevivedAt: '', messageCount: 1, section: 'inbox', activeChildrenCount: 0 },
    }));
    map.set('sibling', makeThreadState('sibling', {
      meta: { id: 'sibling', title: 'Sibling', channel: 'claude_code', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:01Z', status: 'waiting', codingAgentProposed: true, codingAgentRequiresRestart: false, codingAgentIsExternalRepo: false, codingAgentApplying: false, lastRevivedAt: '', messageCount: 1, section: 'inbox', activeChildrenCount: 0 },
    }));
    map.set('elsewhere', makeThreadState('elsewhere', {
      meta: { id: 'elsewhere', title: 'Elsewhere', channel: 'chat', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:00Z', status: 'idle', codingAgentProposed: false, codingAgentRequiresRestart: false, codingAgentIsExternalRepo: false, codingAgentApplying: false, lastRevivedAt: '', messageCount: 0, section: 'archived', activeChildrenCount: 0 },
    }));
    threadMap.value = map;
    focusThread('parent');

    const pending = handleArchiveThread('parent');
    expect(focusedThreadId.value).toBe('sibling'); // optimistic focus moved

    // User navigates somewhere else BEFORE the API resolves.
    focusThread('elsewhere');
    expect(focusedThreadId.value).toBe('elsewhere');

    rejectApi(new Error('boom'));
    await pending;

    // Rollback must respect user's choice — stay on 'elsewhere'.
    expect(focusedThreadId.value).toBe('elsewhere');
  });

  it('restores section + codingAgentProposed and shows a toast when the API rejects', async () => {
    (archiveThread as ReturnType<typeof vi.fn>).mockRejectedValueOnce(new Error('boom'));

    const map = new Map<string, ThreadState>();
    map.set('parent', makeThreadState('parent', {
      meta: { id: 'parent', title: 'Parent', channel: 'claude_code', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:01Z', status: 'waiting', codingAgentProposed: true, codingAgentRequiresRestart: false, codingAgentIsExternalRepo: false, codingAgentApplying: false, lastRevivedAt: '', messageCount: 1, section: 'inbox', activeChildrenCount: 0 },
    }));
    map.set('child', makeThreadState('child', {
      meta: { id: 'child', title: 'Child', channel: 'claude_code', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:00Z', status: 'waiting', codingAgentProposed: true, codingAgentRequiresRestart: false, codingAgentIsExternalRepo: false, codingAgentApplying: false, lastRevivedAt: '', messageCount: 1, section: 'inbox', activeChildrenCount: 0, parentThreadId: 'parent' },
    }));
    threadMap.value = map;
    focusThread('parent');
    toasts.value = [];

    await handleArchiveThread('parent');

    expect(threadMap.value.get('parent')?.meta.section).toBe('inbox');
    expect(threadMap.value.get('child')?.meta.section).toBe('inbox');
    expect(threadMap.value.get('parent')?.meta.codingAgentProposed).toBe(true);
    expect(threadMap.value.get('child')?.meta.codingAgentProposed).toBe(true);
    expect(archivingThreadIds.value.has('parent')).toBe(false);
    expect(toasts.value.some(t => t.type === 'error')).toBe(true);
  });
});

// ---------------------------------------------------------------------------
// handleArchiveThread — stale loadAllThreads response after optimistic flip
// ---------------------------------------------------------------------------
// Bug: on iOS Safari PWA resume the SSE reconnect fires resyncLoadedThreads,
// which kicks off a GET /api/v1/threads. If the user clicks Archive while that
// GET is in flight, the GET's pre-archive snapshot lands AFTER the optimistic
// flip and upsertThread silently overwrites meta.section back to 'inbox' —
// the row flickers back into Review until the SSE ThreadArchived event
// finally confirms the move. The fix mirrors the composeEditedAt staleness
// guard: a request that started before the local flip is by definition stale
// wrt section + codingAgentProposed.
// ---------------------------------------------------------------------------

describe('handleArchiveThread — stale loadAllThreads response', () => {
  it('keeps section archived when a GET issued before the flip lands after with the pre-archive snapshot', async () => {
    let resolveApi: (v: { archived: string[] }) => void = () => {};
    (archiveThread as ReturnType<typeof vi.fn>).mockImplementationOnce(
      () => new Promise<{ archived: string[] }>(r => { resolveApi = r; }),
    );

    const map = new Map<string, ThreadState>();
    map.set('parent', makeThreadState('parent', {
      meta: { id: 'parent', title: 'Parent', channel: 'claude_code', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:01Z', status: 'waiting', codingAgentProposed: true, codingAgentRequiresRestart: false, codingAgentIsExternalRepo: false, codingAgentApplying: false, lastRevivedAt: '', messageCount: 1, section: 'inbox', activeChildrenCount: 0 },
    }));
    map.set('sibling', makeThreadState('sibling', {
      meta: { id: 'sibling', title: 'Sibling', channel: 'chat', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:00Z', status: 'idle', codingAgentProposed: false, codingAgentRequiresRestart: false, codingAgentIsExternalRepo: false, codingAgentApplying: false, lastRevivedAt: '', messageCount: 1, section: 'inbox', activeChildrenCount: 0 },
    }));
    threadMap.value = map;
    focusThread('parent');

    // The stale GET went out BEFORE the user clicked Archive. Sleep so
    // Date.now() advances past `requestStartedAt` before the flip lands —
    // the production race only matters when `flip > requestStartedAt`.
    const requestStartedAt = Date.now();
    await new Promise(r => setTimeout(r, 5));

    // User clicks Archive — optimistic flip to 'archived'.
    const pending = handleArchiveThread('parent');
    expect(threadMap.value.get('parent')?.meta.section).toBe('archived');
    expect(threadMap.value.get('parent')?.meta.codingAgentProposed).toBe(false);

    // The stale GET response lands now, carrying the pre-archive snapshot.
    upsertThread(threadMap.value, {
      thread_id: 'parent',
      title: 'Parent',
      channel: 'claude_code',
      last_activity: '2026-01-01T00:00:01Z',
      created_at: '',
      message_count: 1,
      section: 'inbox',              // stale
      status: 'waiting',
      coding_agent_proposed: true,   // stale
    } as any, false, requestStartedAt);

    // Optimistic flip must survive — no flicker back to Review.
    expect(threadMap.value.get('parent')?.meta.section).toBe('archived');
    expect(threadMap.value.get('parent')?.meta.codingAgentProposed).toBe(false);

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
      meta: { id: 'parent', title: 'Parent', channel: 'claude_code', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:02Z', status: 'waiting', codingAgentProposed: true, codingAgentRequiresRestart: false, codingAgentIsExternalRepo: false, codingAgentApplying: false, lastRevivedAt: '', messageCount: 1, section: 'inbox', activeChildrenCount: 0 },
    }));
    map.set('child', makeThreadState('child', {
      meta: { id: 'child', title: 'Child', channel: 'claude_code', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:01Z', status: 'waiting', codingAgentProposed: true, codingAgentRequiresRestart: false, codingAgentIsExternalRepo: false, codingAgentApplying: false, lastRevivedAt: '', messageCount: 1, section: 'inbox', activeChildrenCount: 0, parentThreadId: 'parent' },
    }));
    threadMap.value = map;
    focusThread('parent');

    const requestStartedAt = Date.now();
    await new Promise(r => setTimeout(r, 5));

    const pending = handleArchiveThread('parent');

    // Stale GET response lands carrying the descendant's pre-archive snapshot.
    upsertThread(threadMap.value, {
      thread_id: 'child',
      title: 'Child',
      channel: 'claude_code',
      last_activity: '2026-01-01T00:00:01Z',
      created_at: '',
      message_count: 1,
      section: 'inbox',
      status: 'waiting',
      coding_agent_proposed: true,
      parent_thread_id: 'parent',
    } as any, false, requestStartedAt);

    expect(threadMap.value.get('child')?.meta.section).toBe('archived');
    expect(threadMap.value.get('child')?.meta.codingAgentProposed).toBe(false);

    resolveApi({ archived: ['parent', 'child'] });
    await pending;
  });

  it('does not block a fresh GET issued after the flip from updating section', async () => {
    // The guard must only suppress STALE responses — a refresh started after
    // the archive (e.g. user navigates back to the list later) must still
    // be allowed to update the row's section if the backend disagrees.
    let resolveApi: (v: { archived: string[] }) => void = () => {};
    (archiveThread as ReturnType<typeof vi.fn>).mockImplementationOnce(
      () => new Promise<{ archived: string[] }>(r => { resolveApi = r; }),
    );

    const map = new Map<string, ThreadState>();
    map.set('parent', makeThreadState('parent', {
      meta: { id: 'parent', title: 'Parent', channel: 'chat', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:01Z', status: 'idle', codingAgentProposed: false, codingAgentRequiresRestart: false, codingAgentIsExternalRepo: false, codingAgentApplying: false, lastRevivedAt: '', messageCount: 1, section: 'inbox', activeChildrenCount: 0 },
    }));
    threadMap.value = map;
    focusThread('parent');

    const pending = handleArchiveThread('parent');
    expect(threadMap.value.get('parent')?.meta.section).toBe('archived');

    // Now a FRESH GET (started after the flip) lands. In the real world this
    // would carry section='archived' once the server has processed the POST.
    await new Promise(r => setTimeout(r, 5));
    const freshRequestStartedAt = Date.now();

    upsertThread(threadMap.value, {
      thread_id: 'parent',
      title: 'Parent',
      channel: 'chat',
      last_activity: '2026-01-01T00:00:01Z',
      created_at: '',
      message_count: 1,
      section: 'archived',
      status: 'idle',
      coding_agent_proposed: false,
    } as any, false, freshRequestStartedAt);

    expect(threadMap.value.get('parent')?.meta.section).toBe('archived');

    resolveApi({ archived: ['parent'] });
    await pending;
  });
});

// ---------------------------------------------------------------------------
// handleArchiveThread — stale SSE aggregate after optimistic flip
// ---------------------------------------------------------------------------
// Bug: every persisted SSE event carries an `aggregate` (backend projection
// snapshot at emit time). The backend cascade runs `stop_agent` -> emits
// CodingAgentIdled -> then ThreadArchived. CodingAgentIdled's aggregate is
// the PRE-archive snapshot (section='inbox'); ThreadArchived's aggregate is
// the POST-archive snapshot (section='archived'). applyAggregateToMeta
// overwrites meta.section unconditionally, so the optimistic flip is reverted
// by the first SSE and restored by the second — bouncing the row between
// Review and Archive. Neighbours visibly reorder twice during the bounce.
// ---------------------------------------------------------------------------

describe('handleArchiveThread — stale SSE aggregate', () => {
  it('keeps section archived when an SSE event arrives with a pre-archive aggregate', async () => {
    let resolveApi: (v: { archived: string[] }) => void = () => {};
    (archiveThread as ReturnType<typeof vi.fn>).mockImplementationOnce(
      () => new Promise<{ archived: string[] }>(r => { resolveApi = r; }),
    );

    const map = new Map<string, ThreadState>();
    map.set('t', makeThreadState('t', {
      meta: { id: 't', title: 'CC thread', channel: 'claude_code', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:01Z', status: 'idle', codingAgentProposed: false, codingAgentRequiresRestart: false, codingAgentIsExternalRepo: false, codingAgentApplying: false, lastRevivedAt: '', messageCount: 1, section: 'inbox', activeChildrenCount: 0 },
    }));
    threadMap.value = map;
    focusThread('t');

    // User clicks Archive — optimistic flip to 'archived'.
    const pending = handleArchiveThread('t');
    expect(threadMap.value.get('t')?.meta.section).toBe('archived');

    // Backend's stop_agent emits CodingAgentIdled BEFORE the ThreadArchived
    // emit. Its aggregate is the PRE-archive snapshot (section='inbox',
    // coding_agent_proposed=false). Without the SSE archive-race guard,
    // applyAggregateToMeta would overwrite section back to 'inbox' and the
    // row would briefly fly back to Review.
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
        codingAgentProposed: false,
        codingAgentRequiresRestart: false,
        codingAgentIsExternalRepo: false,
        codingAgentApplying: false,
        codingAgentHasDiff: false,
        isSaved: false,
        hasResponse: true,
        lastRevivedAt: null,
        parentThreadId: null,
        parentThreadTitle: null,
        state: 'active',
      },
    });

    expect(threadMap.value.get('t')?.meta.section).toBe('archived');
    expect(threadMap.value.get('t')?.meta.codingAgentProposed).toBe(false);

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
      meta: { id: 'parent', title: 'Parent', channel: 'claude_code', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:02Z', status: 'idle', codingAgentProposed: true, codingAgentRequiresRestart: false, codingAgentIsExternalRepo: false, codingAgentApplying: false, lastRevivedAt: '', messageCount: 1, section: 'inbox', activeChildrenCount: 0 },
    }));
    map.set('child', makeThreadState('child', {
      meta: { id: 'child', title: 'Child', channel: 'claude_code', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:01Z', status: 'idle', codingAgentProposed: true, codingAgentRequiresRestart: false, codingAgentIsExternalRepo: false, codingAgentApplying: false, lastRevivedAt: '', messageCount: 1, section: 'inbox', activeChildrenCount: 0, parentThreadId: 'parent' },
    }));
    threadMap.value = map;
    focusThread('parent');

    const pending = handleArchiveThread('parent');
    expect(threadMap.value.get('parent')?.meta.section).toBe('archived');
    expect(threadMap.value.get('child')?.meta.section).toBe('archived');

    // SSE for the child's cascade carries the descendant's pre-archive
    // aggregate. The guard must keep its optimistic flip intact too.
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
        codingAgentProposed: true,
        codingAgentRequiresRestart: false,
        codingAgentIsExternalRepo: false,
        codingAgentApplying: false,
        codingAgentHasDiff: false,
        isSaved: false,
        hasResponse: true,
        lastRevivedAt: null,
        parentThreadId: 'parent',
        parentThreadTitle: 'Parent',
        state: 'active',
      },
    });

    expect(threadMap.value.get('child')?.meta.section).toBe('archived');
    expect(threadMap.value.get('child')?.meta.codingAgentProposed).toBe(false);

    resolveApi({ archived: ['parent', 'child'] });
    await pending;
  });

  it('lets a post-flip aggregate through once archivingThreadIds clears', async () => {
    // The guard must only suppress aggregates that arrived during the
    // in-flight archive. Once the API resolves and archivingThreadIds is
    // cleared, subsequent SSE events apply their aggregate normally.
    (archiveThread as ReturnType<typeof vi.fn>).mockResolvedValueOnce({ archived: ['t'] });

    const map = new Map<string, ThreadState>();
    map.set('t', makeThreadState('t', {
      meta: { id: 't', title: 'Thread', channel: 'chat', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:01Z', status: 'idle', codingAgentProposed: false, codingAgentRequiresRestart: false, codingAgentIsExternalRepo: false, codingAgentApplying: false, lastRevivedAt: '', messageCount: 1, section: 'inbox', activeChildrenCount: 0 },
    }));
    threadMap.value = map;
    focusThread('t');

    await handleArchiveThread('t');
    expect(archivingThreadIds.value.has('t')).toBe(false);

    // After archive completes, an unrelated late SSE arrives with the real
    // backend aggregate. Section should follow the aggregate.
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
        codingAgentProposed: false,
        codingAgentRequiresRestart: false,
        codingAgentIsExternalRepo: false,
        codingAgentApplying: false,
        codingAgentHasDiff: false,
        isSaved: false,
        hasResponse: false,
        lastRevivedAt: null,
        parentThreadId: null,
        parentThreadTitle: null,
        state: 'active',
      },
    });

    expect(threadMap.value.get('t')?.meta.section).toBe('inbox');
  });
});


// ---------------------------------------------------------------------------
// handleArchiveThread — error toast for structured 409 rejections
// ---------------------------------------------------------------------------
// Backend's `POST /api/v1/threads/archive` returns 409 with a structured
// `{reason, parent_status?, blocking?}` body. Without the formatter the toast
// would show the bare `httpCode + reason` from `ApiError.message` — which for
// the archive endpoint is `"409 descendants_blocking"` at best and `"409 "`
// (with empty `statusText` and no `body.error`) at worst. Neither tells the
// user what they need to do.

describe('handleArchiveThread — 409 error toasts', () => {
  beforeEach(() => {
    toasts.value = [];
  });

  function seedReviewThread(): void {
    const map = new Map<string, ThreadState>();
    map.set('parent', makeThreadState('parent', {
      meta: { id: 'parent', title: 'Parent', channel: 'claude_code', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:01Z', status: 'waiting', codingAgentProposed: true, codingAgentRequiresRestart: false, codingAgentIsExternalRepo: false, codingAgentApplying: false, lastRevivedAt: '', messageCount: 1, section: 'inbox', activeChildrenCount: 0 },
    }));
    threadMap.value = map;
    focusThread('parent');
  }

  async function archiveAndGetToast(err: unknown): Promise<string | undefined> {
    (archiveThread as ReturnType<typeof vi.fn>).mockRejectedValueOnce(err);
    await handleArchiveThread('parent');
    return toasts.value.find(t => t.type === 'error')?.message;
  }

  it('formats descendants_blocking (single busy sub-thread) as actionable text', async () => {
    const { ApiError } = await import('../../api/client');
    seedReviewThread();
    const message = await archiveAndGetToast(new ApiError(409, 'descendants_blocking', {
      reason: 'descendants_blocking',
      blocking: [{ thread_id: 'child', status: 'running', has_pending_changes: false }],
    }));
    expect(message).toBe("Can't archive yet — a sub-thread is still busy");
  });

  it('formats descendants_blocking (multiple busy sub-threads) with the count', async () => {
    const { ApiError } = await import('../../api/client');
    seedReviewThread();
    const message = await archiveAndGetToast(new ApiError(409, 'descendants_blocking', {
      reason: 'descendants_blocking',
      blocking: [
        { thread_id: 'child-1', status: 'running', has_pending_changes: false },
        { thread_id: 'child-2', status: 'waiting', has_pending_changes: true },
        { thread_id: 'child-3', status: 'running', has_pending_changes: false },
      ],
    }));
    expect(message).toBe("Can't archive yet — 3 sub-threads are still busy");
  });

  it('formats parent_not_archivable with running parent', async () => {
    const { ApiError } = await import('../../api/client');
    seedReviewThread();
    const message = await archiveAndGetToast(new ApiError(409, 'parent_not_archivable', {
      reason: 'parent_not_archivable',
      parent_status: 'running',
      has_pending_changes: false,
    }));
    expect(message).toBe("Can't archive yet — this thread is still running");
  });

  it('formats parent_not_archivable when parent is already archived', async () => {
    // The OR in `classify_archive_decision` rejects when archive_state is
    // already 'archived' even if status is idle — parent_status comes back
    // as something other than 'running' in that case.
    const { ApiError } = await import('../../api/client');
    seedReviewThread();
    const message = await archiveAndGetToast(new ApiError(409, 'parent_not_archivable', {
      reason: 'parent_not_archivable',
      parent_status: 'idle',
      has_pending_changes: false,
    }));
    expect(message).toBe('This thread is already archived');
  });

  it('formats parent_has_pending_changes with the Apply/Discard hint', async () => {
    // In-workspace CC thread with a pending change is no longer archivable —
    // the user must Apply or Discard first. Toast must say so explicitly,
    // otherwise the Archive click looks like it silently failed.
    const { ApiError } = await import('../../api/client');
    seedReviewThread();
    const message = await archiveAndGetToast(new ApiError(409, 'parent_has_pending_changes', {
      reason: 'parent_has_pending_changes',
    }));
    expect(message).toBe("Can't archive — apply or discard the pending change first");
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
