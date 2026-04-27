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

import { focusedThreadId, threadMap, mobileView, threadDrawerOpen, ccPendingModel, ccPendingReasoningEffort, resetCCPendingPreferences } from '../store';
import type { ThreadState, ThreadMeta } from '../thread-events';
import {
  focusThread,
  unfocusThread,
  handlePinThread,
  handleUnpinThread,
  handleDismissThread,
} from './threads';
import { scrolledUp, notAtTop, getResizeMode } from '../../components/chat/scrollState';
import { drawerOpen } from '../../components/layout/Drawer';

import { loadAllThreads, ensureThreadInMap, upsertThread } from './thread-loading';
import { threadsLoaded, generatedTitleIds } from '../store';

// Mock the API module
vi.mock('../../api/threads', () => ({
  fetchThreads: vi.fn(),
  fetchThreadEvents: vi.fn().mockResolvedValue([]),
  fetchThreadMessages: vi.fn(),
  pinThread: vi.fn().mockResolvedValue(undefined),
  unpinThread: vi.fn().mockResolvedValue(undefined),
  dismissThread: vi.fn().mockResolvedValue(undefined),
}));

vi.mock('../../components/chat/promptFocus', () => ({
  focusPromptNow: vi.fn(),
  focusIfNeeded: vi.fn(),
  composeHandlers: vi.fn(),
}));

vi.mock('../../api/client', () => ({
  API_BASE: '',
  submitChat: vi.fn().mockResolvedValue(undefined),
  cancelChat: vi.fn(),
  cancelClaudeCode: vi.fn(),
  interruptClaudeCode: vi.fn(),
}));

import { fetchThreads } from '../../api/threads';
import { focusPromptNow } from '../../components/chat/promptFocus';

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function makeThreadState(id: string, overrides: Partial<Omit<ThreadState, 'meta'>> & { meta?: Partial<ThreadMeta> } = {}): ThreadState {
  return {
    meta: {
      id,
      title: `Thread ${id}`,
      channel: 'chat',
      initiator: 'user',
      pinned: false,
      createdAt: '2026-01-01T00:00:00Z',
      updatedAt: '2026-01-01T00:00:00Z',
      unread: false,
      status: 'idle',
      ccHasChanges: false,
      ccRequiresRestart: false,
      ccIsExternalRepo: false,
      ccApplying: false,
      lastRevivedAt: '',
      messageCount: 0,
      section: 'default',
      activeChildrenCount: 0,
      totalChildrenCount: 0,
      ...(overrides.meta || {}),
    },
    events: overrides.events || new Map(),
    streamingBuffer: overrides.streamingBuffer || '',
    eventsLoaded: overrides.eventsLoaded || false,
    eventsLoadFailed: overrides.eventsLoadFailed ?? false,
    lastDbSeq: overrides.lastDbSeq ?? 0,
    pendingUserMessages: overrides.pendingUserMessages || [],
  };
}

// ---------------------------------------------------------------------------
// Setup
// ---------------------------------------------------------------------------

beforeEach(() => {
  threadMap.value = new Map();
  focusedThreadId.value = null;
  mobileView.value = 'thread';
  threadDrawerOpen.value = false;
  drawerOpen.value = false;
  resetCCPendingPreferences();
  generatedTitleIds.clear();
  localStorage.removeItem('lucidos-focused-thread');
});

// ---------------------------------------------------------------------------
// focusThread / unfocusThread
// ---------------------------------------------------------------------------

describe('focusThread', () => {
  it('sets focusedThreadId', () => {
    focusThread('t1');
    expect(focusedThreadId.value).toBe('t1');
  });

  it('resets scrolledUp via scrollToBottom — notAtTop left to scroll listener', () => {
    scrolledUp.value = true;
    notAtTop.value = true;
    focusThread('t1');
    expect(scrolledUp.value).toBe(false);
    // notAtTop must NOT be manually reset — the scroll listener (syncNotAtTop)
    // owns it exclusively. Manual resets cause the chevron to disappear when
    // no scroll event fires (e.g. re-focusing the same thread).
    expect(notAtTop.value).toBe(true);
  });

  it('re-focusing the same thread does not hide the scroll-to-top chevron', () => {
    // Bug: clicking the already-focused thread in the drawer called focusThread()
    // which reset notAtTop=false. Since scrollTop didn't change (same content),
    // no scroll event fired, and the chevron never came back.
    notAtTop.value = true;
    scrolledUp.value = false;
    focusThread('t1');
    focusThread('t1'); // re-focus same thread
    expect(notAtTop.value).toBe(true);
  });

  it('suppresses ResizeObserver so content rendering does not set scrolledUp', () => {
    // Bug: focusThread only set scrolledUp=false but didn't suppress ResizeObserver.
    // When thread content rendered, ResizeObserver fired (not at bottom) → scrolledUp=true
    // → useAutoScroll skipped the scroll-to-bottom.
    // Defer rAF callbacks so suppression flag is still active after focusThread().
    const origRAF = globalThis.requestAnimationFrame;
    (globalThis as any).requestAnimationFrame = (_cb: any) => { return 0; };
    try {
      scrolledUp.value = true;
      focusThread('t1');
      expect(getResizeMode()).toBe('scroll');
    } finally {
      (globalThis as any).requestAnimationFrame = origRAF;
    }
  });

  it('navigates to thread pane on mobile', () => {
    // Bug: toast onClick handlers call focusThread() but the user stays on
    // whichever pane they were on. On mobile, focusing a thread must also
    // switch to the thread pane so it becomes visible.
    const origWidth = globalThis.innerWidth;
    (globalThis as any).innerWidth = 375; // mobile
    try {
      mobileView.value = 'threads'; // user is on thread list pane
      focusThread('t1');
      expect(mobileView.value).toBe('thread');
    } finally {
      (globalThis as any).innerWidth = origWidth;
    }
  });

  it('navigates to thread pane on mobile from content pane', () => {
    const origWidth = globalThis.innerWidth;
    (globalThis as any).innerWidth = 375;
    try {
      mobileView.value = 'content';
      focusThread('t1');
      expect(mobileView.value).toBe('thread');
    } finally {
      (globalThis as any).innerWidth = origWidth;
    }
  });

  it('closes drawers when navigating to thread pane on mobile', () => {
    const origWidth = globalThis.innerWidth;
    (globalThis as any).innerWidth = 375;
    try {
      mobileView.value = 'threads';
      drawerOpen.value = true;
      focusThread('t1');
      expect(drawerOpen.value).toBe(false);
    } finally {
      (globalThis as any).innerWidth = origWidth;
    }
  });

  it('does not change mobileView on desktop', () => {
    const origWidth = globalThis.innerWidth;
    (globalThis as any).innerWidth = 1024; // desktop
    try {
      mobileView.value = 'threads';
      focusThread('t1');
      // On desktop, mobileView is unused — must not be mutated
      expect(mobileView.value).toBe('threads');
    } finally {
      (globalThis as any).innerWidth = origWidth;
    }
  });
});

describe('unfocusThread', () => {
  it('clears focusedThreadId', () => {
    focusThread('t1');
    unfocusThread();
    expect(focusedThreadId.value).toBeNull();
  });
});

// ---------------------------------------------------------------------------
// Per-thread CC preferences — reset on thread switch
// ---------------------------------------------------------------------------

describe('CC pending preferences reset on thread switch', () => {
  it('focusThread resets pending CC model and reasoning effort', () => {
    ccPendingModel.value = 'opus';
    ccPendingReasoningEffort.value = 'max';

    focusThread('t1');

    expect(ccPendingModel.value).toBeNull();
    expect(ccPendingReasoningEffort.value).toBeNull();
  });

  it('switching between threads resets pending preferences', () => {
    focusThread('t1');
    ccPendingModel.value = 'sonnet';
    ccPendingReasoningEffort.value = 'low';

    focusThread('t2');

    expect(ccPendingModel.value).toBeNull();
    expect(ccPendingReasoningEffort.value).toBeNull();
  });

  it('unfocusThread resets pending CC preferences (compose view starts fresh)', () => {
    ccPendingModel.value = 'opus';
    ccPendingReasoningEffort.value = 'max';

    unfocusThread();

    expect(ccPendingModel.value).toBeNull();
    expect(ccPendingReasoningEffort.value).toBeNull();
  });
});

// ---------------------------------------------------------------------------
// focusedThreadId persistence
// ---------------------------------------------------------------------------

describe('focusedThreadId persistence', () => {
  it('saves focusedThreadId to localStorage when focusing a thread', () => {
    focusThread('t1');
    expect(localStorage.getItem('lucidos-focused-thread')).toBe('t1');
  });

  it('clears localStorage when unfocusing', () => {
    focusThread('t1');
    unfocusThread();
    expect(localStorage.getItem('lucidos-focused-thread')).toBeNull();
  });

  it('sendMessage persists new thread ID to localStorage', async () => {
    // Root cause of iOS Safari PWA bug: sendMessage() used to clear localStorage
    // for new threads, so reloading the page lost the focused thread.
    const { sendMessage } = await import('./chat');
    const { connectionStatus } = await import('../store');

    // Simulate connected state
    connectionStatus.value = 'connected';

    // No focused thread — this will create a new one
    focusedThreadId.value = null;
    localStorage.removeItem('lucidos-focused-thread');

    await sendMessage('hello');

    // focusedThreadId should be set to the new thread ID
    expect(focusedThreadId.value).not.toBeNull();
    // localStorage must persist it — this is the bug fix
    expect(localStorage.getItem('lucidos-focused-thread')).toBe(focusedThreadId.value);
  });
});

// ---------------------------------------------------------------------------
// handlePinThread / handleUnpinThread
// ---------------------------------------------------------------------------

describe('handlePinThread', () => {
  it('sets pinned to true in threadMap', async () => {
    const map = new Map<string, ThreadState>();
    map.set('t1', makeThreadState('t1'));
    threadMap.value = map;

    await handlePinThread('t1');

    expect(threadMap.value.get('t1')!.meta.pinned).toBe(true);
  });
});

describe('handleUnpinThread', () => {
  it('sets pinned to false in threadMap', async () => {
    const map = new Map<string, ThreadState>();
    map.set('t1', makeThreadState('t1', { meta: { id: 't1', title: 'Thread t1', channel: 'chat', pinned: true, createdAt: '', updatedAt: '2026-01-01T00:00:00Z', unread: false, status: 'idle', ccHasChanges: false, ccRequiresRestart: false, ccIsExternalRepo: false, ccApplying: false, lastRevivedAt: '', messageCount: 0, section: 'default', activeChildrenCount: 0 } }));
    threadMap.value = map;

    await handleUnpinThread('t1');

    expect(threadMap.value.get('t1')!.meta.pinned).toBe(false);
  });
});

// ---------------------------------------------------------------------------
// loadAllThreads — metadata merge and focus preservation
// ---------------------------------------------------------------------------

describe('loadAllThreads', () => {
  beforeEach(() => {
    threadsLoaded.value = false;
  });

  function mockApiResponse(threads: { thread_id: string; title: string; channel: string; last_activity: string; status?: string; cc_has_changes?: boolean; cc_requires_restart?: boolean; cc_is_external_repo?: boolean; cc_applying?: boolean }[]) {
    (fetchThreads as any).mockResolvedValue({
      pinned: [],
      active_threads: [],
      active: [],
      history: threads.map(t => ({
        ...t,
        created_at: t.last_activity,
        message_count: 1,
        section: 'default',
        status: t.status || 'idle',
        cc_has_changes: t.cc_has_changes || false,
        cc_requires_restart: t.cc_requires_restart || false,
        cc_is_external_repo: t.cc_is_external_repo || false,
        cc_applying: t.cc_applying || false,
        active_children_count: 0,
      })),
    });
  }

  it('updates metadata for threads already in map (SSE skeletons)', async () => {
    // SSE skeleton has stale title but newer updatedAt from live events
    const map = new Map<string, ThreadState>();
    map.set('t1', makeThreadState('t1', {
      meta: { id: 't1', title: '...', channel: 'chat', pinned: false, createdAt: '', updatedAt: '2026-03-15T17:00:00Z', unread: false, status: 'idle', ccHasChanges: false, ccRequiresRestart: false, ccIsExternalRepo: false, ccApplying: false, lastRevivedAt: '', messageCount: 0, section: 'default', activeChildrenCount: 0 },
    }));
    threadMap.value = map;

    mockApiResponse([
      { thread_id: 't1', title: 'Real Title', channel: 'claude_code', last_activity: '2026-03-15T18:30:00Z' },
    ]);

    await loadAllThreads();

    const t1 = threadMap.value.get('t1')!;
    expect(t1.meta.title).toBe('Real Title');
    expect(t1.meta.channel).toBe('claude_code');
    // API time (18:30) is newer than SSE skeleton (17:00) — advances
    expect(t1.meta.updatedAt).toBe('2026-03-15T18:30:00Z');
  });

  it('does not overwrite SSE-generated title with stale API data', async () => {
    // SSE delivered ThreadTitleGenerated → generatedTitleIds has this thread
    const map = new Map<string, ThreadState>();
    map.set('t1', makeThreadState('t1', {
      meta: { id: 't1', title: 'Generated Title', channel: 'chat', pinned: false, createdAt: '', updatedAt: '2026-01-01T00:00:00Z', unread: false, status: 'idle', ccHasChanges: false, ccRequiresRestart: false, ccIsExternalRepo: false, ccApplying: false, lastRevivedAt: '', messageCount: 0, section: 'default', activeChildrenCount: 0 },
    }));
    threadMap.value = map;
    generatedTitleIds.add('t1');

    // API returns stale first-message-truncated title
    mockApiResponse([
      { thread_id: 't1', title: 'we used to have a floating down...', channel: 'chat', last_activity: '2026-03-15T18:30:00Z' },
    ]);

    await loadAllThreads();

    expect(threadMap.value.get('t1')!.meta.title).toBe('Generated Title');
  });

  it('does not overwrite title with placeholder "..."', async () => {
    const map = new Map<string, ThreadState>();
    map.set('t1', makeThreadState('t1', {
      meta: { id: 't1', title: 'Good Title From SSE', channel: 'chat', pinned: false, createdAt: '', updatedAt: '2026-01-01T00:00:00Z', unread: false, status: 'idle', ccHasChanges: false, ccRequiresRestart: false, ccIsExternalRepo: false, ccApplying: false, lastRevivedAt: '', messageCount: 0, section: 'default', activeChildrenCount: 0 },
    }));
    threadMap.value = map;

    mockApiResponse([
      { thread_id: 't1', title: '...', channel: 'chat', last_activity: '2026-03-15T18:30:00Z' },
    ]);

    await loadAllThreads();

    expect(threadMap.value.get('t1')!.meta.title).toBe('Good Title From SSE');
  });

  it('preserves focused thread from localStorage', async () => {
    localStorage.setItem('lucidos-focused-thread', 't2');
    focusedThreadId.value = 't2';

    mockApiResponse([
      { thread_id: 't1', title: 'Thread 1', channel: 'chat', last_activity: '2026-03-15T19:00:00Z' },
      { thread_id: 't2', title: 'Thread 2', channel: 'claude_code', last_activity: '2026-03-15T18:00:00Z' },
    ]);

    await loadAllThreads();

    // Should keep t2 focused (from localStorage), not auto-focus t1 (most recent)
    expect(focusedThreadId.value).toBe('t2');
  });

  it('does not auto-focus any thread when none focused (shows compose view)', async () => {
    mockApiResponse([
      { thread_id: 't1', title: 'Older', channel: 'chat', last_activity: '2026-03-15T10:00:00Z' },
      { thread_id: 't2', title: 'Newer', channel: 'chat', last_activity: '2026-03-15T18:00:00Z' },
    ]);

    await loadAllThreads();

    expect(focusedThreadId.value).toBeNull();
    expect(localStorage.getItem('lucidos-focused-thread')).toBeNull();
  });

  it('API updatedAt only advances forward, never regresses', async () => {
    // SSE skeletons with timestamps from live events
    const map = new Map<string, ThreadState>();
    map.set('t1', makeThreadState('t1', {
      meta: { id: 't1', title: '...', channel: 'chat', pinned: false, createdAt: '', updatedAt: '2026-03-15T19:30:00Z', unread: false, status: 'idle', ccHasChanges: false, ccRequiresRestart: false, ccIsExternalRepo: false, ccApplying: false, lastRevivedAt: '', messageCount: 0, section: 'default', activeChildrenCount: 0 },
    }));
    map.set('t2', makeThreadState('t2', {
      meta: { id: 't2', title: '...', channel: 'chat', pinned: false, createdAt: '', updatedAt: '2026-03-15T15:00:00Z', unread: false, status: 'idle', ccHasChanges: false, ccRequiresRestart: false, ccIsExternalRepo: false, ccApplying: false, lastRevivedAt: '', messageCount: 0, section: 'default', activeChildrenCount: 0 },
    }));
    threadMap.value = map;

    // API: t1 has stale last_activity (older than SSE), t2 has newer last_activity
    mockApiResponse([
      { thread_id: 't1', title: 'Old Thread', channel: 'chat', last_activity: '2026-03-15T10:00:00Z' },
      { thread_id: 't2', title: 'Recent Thread', channel: 'claude_code', last_activity: '2026-03-15T18:00:00Z' },
    ]);

    await loadAllThreads();

    // t1: SSE value (19:30) is newer than API (10:00) — keep SSE value
    expect(threadMap.value.get('t1')!.meta.updatedAt).toBe('2026-03-15T19:30:00Z');
    // t2: API value (18:00) is newer than SSE (15:00) — advance to API value
    expect(threadMap.value.get('t2')!.meta.updatedAt).toBe('2026-03-15T18:00:00Z');
  });

  it('sets threadsLoaded after loading', async () => {
    mockApiResponse([]);
    expect(threadsLoaded.value).toBe(false);
    await loadAllThreads();
    expect(threadsLoaded.value).toBe(true);
  });

  it('populates messageCount from API message_count', async () => {
    (fetchThreads as any).mockResolvedValue({
      pinned: [],
      active_threads: [],
      active: [],
      history: [
        { thread_id: 't1', title: 'Thread 1', channel: 'chat', last_activity: '2026-03-15T18:00:00Z', created_at: '2026-03-15T18:00:00Z', message_count: 5, section: 'default', status: 'idle', cc_has_changes: false, cc_requires_restart: false, cc_is_external_repo: false, cc_applying: false, last_revived_at: null, active_children_count: 0 },
        { thread_id: 't2', title: 'Thread 2', channel: 'claude_code', last_activity: '2026-03-15T17:00:00Z', created_at: '2026-03-15T17:00:00Z', message_count: 12, section: 'default', status: 'idle', cc_has_changes: false, cc_requires_restart: false, cc_is_external_repo: false, cc_applying: false, last_revived_at: null, active_children_count: 0 },
      ],
    });

    await loadAllThreads();

    expect(threadMap.value.get('t1')!.meta.messageCount).toBe(5);
    expect(threadMap.value.get('t2')!.meta.messageCount).toBe(12);
  });

  it('updates messageCount on existing threads from API', async () => {
    const map = new Map<string, ThreadState>();
    map.set('t1', makeThreadState('t1', {
      meta: { id: 't1', title: '...', channel: 'chat', pinned: false, createdAt: '', updatedAt: '2026-03-15T19:00:00Z', unread: false, status: 'idle', ccHasChanges: false, ccRequiresRestart: false, ccIsExternalRepo: false, ccApplying: false, lastRevivedAt: '', messageCount: 0, section: 'default', activeChildrenCount: 0 },
    }));
    threadMap.value = map;

    (fetchThreads as any).mockResolvedValue({
      pinned: [],
      active_threads: [],
      active: [],
      history: [
        { thread_id: 't1', title: 'Thread 1', channel: 'chat', last_activity: '2026-03-15T18:00:00Z', created_at: '2026-03-15T18:00:00Z', message_count: 7, section: 'default', status: 'idle', cc_has_changes: false, cc_requires_restart: false, cc_is_external_repo: false, cc_applying: false, last_revived_at: null, active_children_count: 0 },
      ],
    });

    await loadAllThreads();

    expect(threadMap.value.get('t1')!.meta.messageCount).toBe(7);
  });

  it('marks active threads from API active set', async () => {
    (fetchThreads as any).mockResolvedValue({
      pinned: [],
      active_threads: [
        { thread_id: 't1', title: 'Active', channel: 'chat', last_activity: '2026-03-15T18:00:00Z', created_at: '2026-03-15T18:00:00Z', message_count: 1, section: 'default', active_children_count: 0, status: 'running', cc_has_changes: false, cc_requires_restart: false, cc_is_external_repo: false, cc_applying: false, last_revived_at: null },
      ],
      active: ['t1'],
      history: [
        { thread_id: 't2', title: 'Idle', channel: 'chat', last_activity: '2026-03-15T17:00:00Z', created_at: '2026-03-15T17:00:00Z', message_count: 3, section: 'default', active_children_count: 0, status: 'idle', cc_has_changes: false, cc_requires_restart: false, cc_is_external_repo: false, cc_applying: false, last_revived_at: null },
      ],
    });

    await loadAllThreads();

    expect(threadMap.value.get('t1')!.meta.status).toBe('running');
    expect(threadMap.value.get('t2')!.meta.status).toBe('idle');
  });

  it('threadsLoaded is false before loadAllThreads resolves', async () => {
    expect(threadsLoaded.value).toBe(false);
    // SSE skeletons exist in threadMap but threadsLoaded is still false
    const map = new Map<string, ThreadState>();
    map.set('t1', makeThreadState('t1'));
    threadMap.value = map;
    expect(threadsLoaded.value).toBe(false);
    expect(threadMap.value.size).toBe(1);
  });

  it('preserves API channel on initial load — thread_summaries source is authoritative', async () => {
    // API returns channel='trigger' (authoritative from thread_summaries).
    // Events may carry different channels (e.g., a follow-up MessageReceived with
    // channel='chat'), but the API value must win on initial load.
    (fetchThreads as any).mockResolvedValue({
      pinned: [],
      active_threads: [
        { thread_id: 't1', title: 'Trigger Run', channel: 'trigger', last_activity: '2026-03-15T18:00:00Z', created_at: '2026-03-15T18:00:00Z', message_count: 1, section: 'default', status: 'running', cc_has_changes: false, cc_requires_restart: false, cc_is_external_repo: false, cc_applying: false, last_revived_at: null, active_children_count: 0 },
      ],
      active: ['t1'],
      history: [],
    });

    // Event has a different channel — this must NOT overwrite the API channel
    const { fetchThreadEvents } = await import('../../api/threads');
    (fetchThreadEvents as any).mockResolvedValue([
      {
        sequence: 1,
        event_type: 'MessageReceived',
        payload: { text: 'follow-up', channel: 'chat' },
        created: '2026-03-15T18:00:00Z',
        event_id: 'e1',
      },
    ]);

    await loadAllThreads();

    // API channel is preserved — events don't overwrite on initial load
    const thread = threadMap.value.get('t1')!;
    expect(thread.meta.channel).toBe('trigger');
  });
});

// ---------------------------------------------------------------------------
// ensureThreadInMap — search result click bootstrapping
// ---------------------------------------------------------------------------

describe('ensureThreadInMap', () => {
  it('creates a skeleton ThreadState for a thread not in the map', async () => {
    threadsLoaded.value = true;

    const { fetchThreadEvents } = await import('../../api/threads');
    (fetchThreadEvents as any).mockResolvedValue([
      {
        sequence: 1,
        event_type: 'MessageReceived',
        payload: { text: 'hello world', channel: 'chat' },
        created: '2026-03-15T18:00:00Z',
        event_id: 'e1',
      },
    ]);

    expect(threadMap.value.has('search-t1')).toBe(false);

    await ensureThreadInMap({
      thread_id: 'search-t1',
      title: 'Search Result Thread',
      channel: 'chat',
      initiator: 'user',
      last_activity: '2026-03-15T18:00:00Z',
      created_at: '2026-03-15T18:00:00Z',
      message_count: 3,
      section: 'default',
      active_children_count: 0,
      total_children_count: 0,
      status: 'idle',
      cc_has_changes: false,
      cc_requires_restart: false,
      cc_is_external_repo: false,
      cc_applying: false, last_revived_at: null,
    });

    const thread = threadMap.value.get('search-t1');
    expect(thread).toBeDefined();
    expect(thread!.meta.title).toBe('Search Result Thread');
    expect(thread!.meta.channel).toBe('chat');
    expect(thread!.meta.messageCount).toBe(3);
    expect(thread!.eventsLoaded).toBe(true);
  });

  it('does not overwrite a thread already in the map', async () => {
    const map = new Map<string, ThreadState>();
    map.set('t1', makeThreadState('t1', {
      meta: { id: 't1', title: 'Existing Title', channel: 'claude_code', initiator: 'user', pinned: true, createdAt: '', updatedAt: '2026-01-01T00:00:00Z', unread: false, status: 'idle', ccHasChanges: false, ccRequiresRestart: false, ccIsExternalRepo: false, ccApplying: false, lastRevivedAt: '', messageCount: 5, section: 'default', activeChildrenCount: 0 },
      eventsLoaded: true,
    }));
    threadMap.value = map;

    await ensureThreadInMap({
      thread_id: 't1',
      title: 'Search Title',
      channel: 'chat',
      initiator: 'user',
      last_activity: '2026-03-15T18:00:00Z',
      created_at: '2026-03-15T18:00:00Z',
      message_count: 1,
      section: 'default',
      active_children_count: 0,
      total_children_count: 0,
      status: 'idle',
      cc_has_changes: false,
      cc_requires_restart: false,
      cc_is_external_repo: false,
      cc_applying: false, last_revived_at: null,
    });

    const thread = threadMap.value.get('t1')!;
    expect(thread.meta.title).toBe('Existing Title');
    expect(thread.meta.pinned).toBe(true);
  });
});

// ---------------------------------------------------------------------------
// Bug: CC thread spawned by chat shows in History instead of Running
// ---------------------------------------------------------------------------

describe('CC thread spawned by chat — status from API is authoritative', () => {
  it('API status always overwrites SSE skeleton status', async () => {
    // Scenario: SSE creates a CC thread skeleton with status='running'
    // (from CodingAgentThreadSpawned). Then loadAllThreads runs and the API doesn't
    // include this thread in the active set (CC session hasn't registered yet).
    // Previously, upsertThread would set status='idle', causing the
    // thread to show in History instead of Running.
    const map = new Map<string, ThreadState>();
    map.set('cc-1', makeThreadState('cc-1', {
      meta: {
        id: 'cc-1',
        title: 'Fix OAuth URLs',
        channel: 'claude_code',
        pinned: false,
        createdAt: '2026-03-19T20:00:00Z',
        updatedAt: '2026-03-19T20:00:00Z',
        unread: false,
        status: 'running',
      ccHasChanges: false,
      ccRequiresRestart: false,
      ccIsExternalRepo: false,
      ccApplying: false,  // Set by SSE skeleton
        lastRevivedAt: '',
        messageCount: 0,
        section: 'default',
        activeChildrenCount: 0,
      },
    }));
    threadMap.value = map;

    // API response includes the CC thread in history but NOT in active set
    // (because the CC session hasn't registered itself yet)
    (fetchThreads as any).mockResolvedValue({
      pinned: [],
      active_threads: [],
      active: [],  // CC thread NOT in active set
      history: [
        { thread_id: 'cc-1', title: 'Fix OAuth URLs', channel: 'claude_code', last_activity: '2026-03-19T20:00:00Z', created_at: '2026-03-19T20:00:00Z', message_count: 1, section: 'default', active_children_count: 0, status: 'idle', cc_has_changes: false, cc_requires_restart: false, cc_is_external_repo: false, cc_applying: false, last_revived_at: null },
      ],
    });

    await loadAllThreads();

    // status must NOT be downgraded — events not loaded yet, SSE skeleton's running status takes precedence
    expect(threadMap.value.get('cc-1')!.meta.status).toBe('idle');
  });

  it('API status is applied even if SSE status differs', async () => {
    // Scenario: CC thread was running, engine restarted, events loaded show
    // a terminal state (ResponseAborted). API now reports thread as not active.
    // With eventsLoaded=true, the downgrade is safe — no race window.
    const map = new Map<string, ThreadState>();
    map.set('cc-1', makeThreadState('cc-1', {
      meta: {
        id: 'cc-1',
        title: 'Aborted Session',
        channel: 'claude_code',
        pinned: false,
        createdAt: '2026-03-19T20:00:00Z',
        updatedAt: '2026-03-19T20:00:00Z',
        unread: false,
        status: 'running',
      ccHasChanges: false,
      ccRequiresRestart: false,
      ccIsExternalRepo: false,
      ccApplying: false,  // Was running before restart
        lastRevivedAt: '',
        messageCount: 0,
        section: 'default',
        activeChildrenCount: 0,
      },
      eventsLoaded: true,  // Events have been loaded — downgrade is safe
    }));
    threadMap.value = map;

    (fetchThreads as any).mockResolvedValue({
      pinned: [],
      active_threads: [],
      active: [],  // Thread no longer active after restart
      history: [
        { thread_id: 'cc-1', title: 'Aborted Session', channel: 'claude_code', last_activity: '2026-03-19T20:00:00Z', created_at: '2026-03-19T20:00:00Z', message_count: 1, section: 'unread', status: 'idle', cc_has_changes: false, cc_requires_restart: false, cc_is_external_repo: false, cc_applying: false, last_revived_at: null, active_children_count: 0 },
      ],
    });

    await loadAllThreads();

    // API status is applied — backend is authoritative
    expect(threadMap.value.get('cc-1')!.meta.status).toBe('idle');
  });
});

// ---------------------------------------------------------------------------
// Event replay must not override API status
// ---------------------------------------------------------------------------

describe('event replay must not override API status', () => {
  it('API status=idle is preserved after replaying events ending with CodingAgentPromptSent', async () => {
    // Scenario: CC thread had a CodingAgentPromptSent as the last
    // status-affecting event (session crashed mid-work). The migration backfill
    // correctly set status='idle' in the DB. But event replay in applyEventRows
    // was overriding it to 'running' because the last CodingAgentPromptSent
    // calls updateStatusFromEvent which sets meta.status='running'.
    //
    // This is the exact bug: thread shows "In Progress" in the drawer despite
    // the backend knowing it's idle.
    (fetchThreads as any).mockResolvedValue({
      pinned: [],
      active_threads: [
        {
          thread_id: 'stuck-t1',
          title: 'Stuck Thread',
          channel: 'claude_code',
          last_activity: '2026-03-30T09:00:00Z',
          created_at: '2026-03-28T19:00:00Z',
          message_count: 5,
          section: 'default',
          active_children_count: 0,
          status: 'idle',  // Backend says idle (session is dead)
          cc_has_changes: false,
          cc_requires_restart: false,
          cc_is_external_repo: false,
          cc_applying: false, last_revived_at: null,
        },
      ],
      active: ['stuck-t1'],
      history: [],
    });

    const { fetchThreadEvents } = await import('../../api/threads');
    (fetchThreadEvents as any).mockResolvedValue([
      {
        sequence: 1,
        event_type: 'MessageReceived',
        payload: { text: 'fix the bug', channel: 'claude_code' },
        created: '2026-03-28T19:00:00Z',
        event_id: 'e1',
      },
      {
        sequence: 2,
        event_type: 'SessionStarted',
        payload: { session_id: 's1' },
        created: '2026-03-28T19:00:01Z',
        event_id: 'e2',
      },
      {
        sequence: 3,
        event_type: 'CodingAgentIdled',
        payload: { has_changes: false },
        created: '2026-03-28T19:05:00Z',
        event_id: 'e3',
      },
      // Automated prompt — sets status='running' during replay, but the session is dead.
      {
        sequence: 4,
        event_type: 'CodingAgentPromptSent',
        payload: { text: '(session resumed)' },
        created: '2026-03-30T08:41:00Z',
        event_id: 'e4',
      },
      {
        sequence: 5,
        event_type: 'SessionStarted',
        payload: { session_id: 's2' },
        created: '2026-03-30T08:41:01Z',
        event_id: 'e5',
      },
      // No terminal event — session crashed here
    ]);

    await loadAllThreads();

    const thread = threadMap.value.get('stuck-t1')!;
    // API said 'idle' — event replay must NOT override this to 'running'
    expect(thread.meta.status).toBe('idle');
  });

  it('refreshThreadEvents allows new events to update status', async () => {
    // Scenario: thread was idle, user sends a message while SSE was disconnected.
    // On reconnect, refreshThreadEvents fetches the new MessageReceived from DB.
    // The event must update status to 'running' — it's a real live event, not stale replay.
    const map = new Map<string, ThreadState>();
    map.set('refresh-t1', makeThreadState('refresh-t1', {
      meta: { id: 'refresh-t1', title: 'Refresh Thread', channel: 'claude_code', pinned: false, createdAt: '2026-03-28T19:00:00Z', updatedAt: '2026-03-28T19:00:00Z', unread: false, status: 'idle', ccHasChanges: false, ccRequiresRestart: false, ccIsExternalRepo: false, ccApplying: false, lastRevivedAt: '', messageCount: 1, section: 'default', activeChildrenCount: 0 },
      eventsLoaded: true,
      lastDbSeq: 3,
    }));
    threadMap.value = map;

    const { fetchThreadEvents } = await import('../../api/threads');
    (fetchThreadEvents as any).mockResolvedValue([
      {
        sequence: 4,
        event_type: 'MessageReceived',
        payload: { text: 'new message', channel: 'claude_code' },
        created: '2026-03-30T10:00:00Z',
        event_id: 'e4',
      },
    ]);

    const { refreshThreadEvents } = await import('./thread-loading');
    await refreshThreadEvents('refresh-t1');

    // New event must update status — refreshThreadEvents does NOT preserve stale status
    expect(threadMap.value.get('refresh-t1')!.meta.status).toBe('running');
  });
});

// ---------------------------------------------------------------------------
// Section — focusThread does NOT auto-read (user must click Done/Apply/Discard)
// ---------------------------------------------------------------------------

describe('focusThread — section', () => {
  it('does not mark an unread thread as read when focused', () => {
    const map = new Map<string, ThreadState>();
    map.set('t1', makeThreadState('t1', {
      meta: { id: 't1', title: 'Unread Thread', channel: 'chat', pinned: false, createdAt: '', updatedAt: '2026-01-01T00:00:00Z', unread: false, status: 'idle', ccHasChanges: false, ccRequiresRestart: false, ccIsExternalRepo: false, ccApplying: false, lastRevivedAt: '', messageCount: 0, section: 'unread', activeChildrenCount: 0 },
    }));
    threadMap.value = map;

    focusThread('t1');

    // Section stays 'unread' — no auto-read on focus
    expect(threadMap.value.get('t1')!.meta.section).toBe('unread');
  });

  it('does not update threadMap when section is already default', () => {
    const map = new Map<string, ThreadState>();
    map.set('t1', makeThreadState('t1'));
    threadMap.value = map;

    const mapBefore = threadMap.value;
    focusThread('t1');

    // threadMap reference should not change (no unnecessary signal trigger)
    expect(threadMap.value).toBe(mapBefore);
  });
});

// ---------------------------------------------------------------------------
// Section — loadAllThreads preserves section from API
// ---------------------------------------------------------------------------

describe('loadAllThreads — section', () => {
  beforeEach(() => {
    threadsLoaded.value = false;
  });

  it('populates section from API section', async () => {
    (fetchThreads as any).mockResolvedValue({
      pinned: [],
      active_threads: [],
      active: [],
      history: [
        { thread_id: 't1', title: 'Unread', channel: 'chat', last_activity: '2026-03-15T18:00:00Z', created_at: '2026-03-15T18:00:00Z', message_count: 1, section: 'unread' },
        { thread_id: 't2', title: 'Normal', channel: 'chat', last_activity: '2026-03-15T17:00:00Z', created_at: '2026-03-15T17:00:00Z', message_count: 2, section: 'default' },
      ],
    });

    await loadAllThreads();

    expect(threadMap.value.get('t1')!.meta.section).toBe('unread');
    expect(threadMap.value.get('t2')!.meta.section).toBe('default');
  });

  it('event replay does not override API section with stale ThreadMarkedUnread', async () => {
    // Scenario: CC session idled (ThreadMarkedUnread persisted), then change was applied
    // (section cleared to 'default' in thread_summaries). But ThreadMarkedRead wasn't
    // persisted (e.g., fix wasn't deployed yet). On reload, event replay must not
    // override the authoritative API section with the stale ThreadMarkedUnread.
    (fetchThreads as any).mockResolvedValue({
      pinned: [],
      active_threads: [
        { thread_id: 't1', title: 'CC Thread', channel: 'claude_code', last_activity: '2026-03-15T18:00:00Z', created_at: '2026-03-15T18:00:00Z', message_count: 1, section: 'default' },
      ],
      active: ['t1'],
      history: [],
    });

    const { fetchThreadEvents } = await import('../../api/threads');
    (fetchThreadEvents as any).mockResolvedValue([
      { sequence: 1, event_type: 'CodingAgentUserMessageSent', payload: { text: 'fix bug' }, created: '2026-01-01T00:00:01Z', event_id: 'e1' },
      { sequence: 2, event_type: 'CodingAgentIdled', payload: { has_changes: true }, created: '2026-01-01T00:00:05Z', event_id: 'e2' },
      { sequence: 3, event_type: 'ThreadMarkedUnread', payload: {}, created: '2026-01-01T00:00:05Z', event_id: 'e3' },
      { sequence: 4, event_type: 'ChangeApplied', payload: { change_id: 'c-1' }, created: '2026-01-01T00:00:10Z', event_id: 'e4' },
      { sequence: 5, event_type: 'SessionEnded', payload: { reason: 'completed' }, created: '2026-01-01T00:00:11Z', event_id: 'e5' },
      // Note: no ThreadMarkedRead event — this is the bug scenario
    ]);

    await loadAllThreads();

    // API says section='default' — event replay of ThreadMarkedUnread must not override it
    expect(threadMap.value.get('t1')!.meta.section).toBe('default');
  });

  it('updates section on existing threads from API', async () => {
    const map = new Map<string, ThreadState>();
    map.set('t1', makeThreadState('t1', {
      meta: { id: 't1', title: '...', channel: 'chat', pinned: false, createdAt: '', updatedAt: '2026-03-15T19:00:00Z', unread: false, status: 'idle', ccHasChanges: false, ccRequiresRestart: false, ccIsExternalRepo: false, ccApplying: false, lastRevivedAt: '', messageCount: 0, section: 'default', activeChildrenCount: 0 },
    }));
    threadMap.value = map;

    (fetchThreads as any).mockResolvedValue({
      pinned: [],
      active_threads: [],
      active: [],
      history: [
        { thread_id: 't1', title: 'Thread 1', channel: 'chat', last_activity: '2026-03-15T18:00:00Z', created_at: '2026-03-15T18:00:00Z', message_count: 1, section: 'unread' },
      ],
    });

    await loadAllThreads();

    expect(threadMap.value.get('t1')!.meta.section).toBe('unread');
  });
});

// ---------------------------------------------------------------------------
// Section — upsertThread must not overwrite updatedAt with stale API value
// ---------------------------------------------------------------------------

describe('upsertThread — updatedAt monotonic', () => {
  it('does not overwrite a newer SSE-derived updatedAt with a stale API last_activity', () => {
    // Scenario: CC session is actively streaming. SSE events have updated
    // meta.updatedAt to 19:31. Then loadAllThreads runs (e.g. on visibility
    // change / resume) and the API returns last_activity=19:21 because the
    // backend projection doesn't update for CC streaming events. The stale
    // API value must NOT overwrite the fresher SSE-derived timestamp.
    const map = new Map<string, ThreadState>();
    map.set('t1', makeThreadState('t1', {
      meta: {
        id: 't1', title: 'CC Thread', channel: 'claude_code', pinned: false,
        createdAt: '2026-03-31T19:21:38Z',
        updatedAt: '2026-03-31T19:31:54Z', // SSE-updated (newer)
        unread: false, status: 'running', ccHasChanges: false,
        ccRequiresRestart: false, ccIsExternalRepo: false, ccApplying: false, lastRevivedAt: '',
        messageCount: 0, section: 'default', activeChildrenCount: 0,
      },
    }));

    upsertThread(map, {
      thread_id: 't1',
      title: 'CC Thread',
      channel: 'claude_code',
      last_activity: '2026-03-31T19:21:38Z', // stale backend value
      created_at: '2026-03-31T19:21:38Z',
      message_count: 1,
      section: 'default',
      status: 'running',
    } as any, false);

    // updatedAt must keep the newer SSE value, not regress to the stale API value
    expect(map.get('t1')!.meta.updatedAt).toBe('2026-03-31T19:31:54Z');
  });

  it('advances updatedAt when API returns a newer last_activity', () => {
    const map = new Map<string, ThreadState>();
    map.set('t1', makeThreadState('t1', {
      meta: {
        id: 't1', title: 'Thread', channel: 'chat', pinned: false,
        createdAt: '2026-03-31T10:00:00Z',
        updatedAt: '2026-03-31T10:00:00Z',
        unread: false, status: 'idle', ccHasChanges: false,
        ccRequiresRestart: false, ccIsExternalRepo: false, ccApplying: false, lastRevivedAt: '',
        messageCount: 0, section: 'default', activeChildrenCount: 0,
      },
    }));

    upsertThread(map, {
      thread_id: 't1',
      title: 'Thread',
      channel: 'chat',
      last_activity: '2026-03-31T12:00:00Z', // newer
      created_at: '2026-03-31T10:00:00Z',
      message_count: 2,
      section: 'default',
      status: 'idle',
    } as any, false);

    // API has a newer timestamp — should advance
    expect(map.get('t1')!.meta.updatedAt).toBe('2026-03-31T12:00:00Z');
  });
});

// ---------------------------------------------------------------------------
// handleDismissThread — focus next review thread
// ---------------------------------------------------------------------------

describe('handleDismissThread', () => {
  it('focuses the next review thread after dismissing', async () => {
    // Set up: t1 (focused, waiting/unread = review), t2 (also waiting/unread = review)
    const map = new Map<string, ThreadState>();
    map.set('t1', makeThreadState('t1', {
      meta: { id: 't1', title: 'Thread t1', channel: 'claude_code', pinned: false, createdAt: '', updatedAt: '2026-01-01T00:00:01Z', unread: true, status: 'waiting', ccHasChanges: true, ccRequiresRestart: false, ccIsExternalRepo: false, ccApplying: false, lastRevivedAt: '', messageCount: 1, section: 'unread', activeChildrenCount: 0 },
    }));
    map.set('t2', makeThreadState('t2', {
      meta: { id: 't2', title: 'Thread t2', channel: 'claude_code', pinned: false, createdAt: '', updatedAt: '2026-01-01T00:00:00Z', unread: true, status: 'waiting', ccHasChanges: true, ccRequiresRestart: false, ccIsExternalRepo: false, ccApplying: false, lastRevivedAt: '', messageCount: 1, section: 'unread', activeChildrenCount: 0 },
    }));
    threadMap.value = map;
    focusThread('t1');

    await handleDismissThread('t1');

    // Should focus t2 (next review thread), not unfocus
    expect(focusedThreadId.value).toBe('t2');
  });

  it('unfocuses and resets mobileView when no more review threads remain', async () => {
    const map = new Map<string, ThreadState>();
    map.set('t1', makeThreadState('t1', {
      meta: { id: 't1', title: 'Thread t1', channel: 'claude_code', pinned: false, createdAt: '', updatedAt: '2026-01-01T00:00:00Z', unread: true, status: 'waiting', ccHasChanges: true, ccRequiresRestart: false, ccIsExternalRepo: false, ccApplying: false, lastRevivedAt: '', messageCount: 1, section: 'unread', activeChildrenCount: 0 },
    }));
    threadMap.value = map;
    focusThread('t1');
    mobileView.value = 'threads';

    await handleDismissThread('t1');

    expect(focusedThreadId.value).toBeNull();
    expect(mobileView.value).toBe('thread');
  });

  it('does not focus prompt when dismissing the last review thread', async () => {
    // Bug: dismissing the last review thread called focusPromptNow(), which
    // opened the keyboard on mobile. The compose view should appear with the
    // prompt unfocused and the header visible.
    const map = new Map<string, ThreadState>();
    map.set('t1', makeThreadState('t1', {
      meta: { id: 't1', title: 'Last review', channel: 'claude_code', pinned: false, createdAt: '', updatedAt: '2026-01-01T00:00:00Z', unread: true, status: 'waiting', ccHasChanges: true, ccRequiresRestart: false, ccIsExternalRepo: false, ccApplying: false, lastRevivedAt: '', messageCount: 1, section: 'unread', activeChildrenCount: 0 },
    }));
    threadMap.value = map;
    focusThread('t1');
    (focusPromptNow as ReturnType<typeof vi.fn>).mockClear();

    await handleDismissThread('t1');

    expect(focusPromptNow).not.toHaveBeenCalled();
    expect(focusedThreadId.value).toBeNull();
  });

  it('focuses the next thread below, not the top one', async () => {
    // 3 review threads sorted by updatedAt desc: t1 (newest), t2 (middle), t3 (oldest)
    // Dismissing t2 should focus t3 (below), not t1 (top)
    const map = new Map<string, ThreadState>();
    map.set('t1', makeThreadState('t1', {
      meta: { id: 't1', title: 'Thread t1', channel: 'claude_code', pinned: false, createdAt: '', updatedAt: '2026-01-01T00:00:02Z', unread: true, status: 'waiting', ccHasChanges: false, ccRequiresRestart: false, ccIsExternalRepo: false, ccApplying: false, lastRevivedAt: '', messageCount: 1, section: 'unread', activeChildrenCount: 0 },
    }));
    map.set('t2', makeThreadState('t2', {
      meta: { id: 't2', title: 'Thread t2', channel: 'claude_code', pinned: false, createdAt: '', updatedAt: '2026-01-01T00:00:01Z', unread: true, status: 'waiting', ccHasChanges: false, ccRequiresRestart: false, ccIsExternalRepo: false, ccApplying: false, lastRevivedAt: '', messageCount: 1, section: 'unread', activeChildrenCount: 0 },
    }));
    map.set('t3', makeThreadState('t3', {
      meta: { id: 't3', title: 'Thread t3', channel: 'claude_code', pinned: false, createdAt: '', updatedAt: '2026-01-01T00:00:00Z', unread: true, status: 'waiting', ccHasChanges: false, ccRequiresRestart: false, ccIsExternalRepo: false, ccApplying: false, lastRevivedAt: '', messageCount: 1, section: 'unread', activeChildrenCount: 0 },
    }));
    threadMap.value = map;
    focusThread('t2');

    await handleDismissThread('t2');

    expect(focusedThreadId.value).toBe('t3');
  });

  it('navigates to next review thread after apply+done (regression: Apply used to skip Done)', async () => {
    // Simulates the fixed flow: after Apply, thread stays in review (section=unread,
    // ccHasChanges=false), Done button appears, clicking Done navigates to next thread.
    // Previously Apply moved the thread to HISTORY immediately, skipping Done.
    const map = new Map<string, ThreadState>();
    // t1: just applied — still in review with Done button (unread, no changes, idle)
    map.set('t1', makeThreadState('t1', {
      meta: { id: 't1', title: 'Applied thread', channel: 'claude_code', pinned: false, createdAt: '', updatedAt: '2026-01-01T00:00:01Z', unread: false, status: 'idle', ccHasChanges: false, ccRequiresRestart: false, ccIsExternalRepo: false, ccApplying: false, lastRevivedAt: '', messageCount: 1, section: 'unread', activeChildrenCount: 0 },
    }));
    // t2: another review thread waiting for attention
    map.set('t2', makeThreadState('t2', {
      meta: { id: 't2', title: 'Pending thread', channel: 'claude_code', pinned: false, createdAt: '', updatedAt: '2026-01-01T00:00:00Z', unread: true, status: 'waiting', ccHasChanges: true, ccRequiresRestart: false, ccIsExternalRepo: false, ccApplying: false, lastRevivedAt: '', messageCount: 1, section: 'unread', activeChildrenCount: 0 },
    }));
    threadMap.value = map;
    focusThread('t1');

    await handleDismissThread('t1');

    // After Done on t1, should navigate to t2 (next review thread)
    expect(focusedThreadId.value).toBe('t2');
  });

  it('keeps thread drawer open on desktop when all reviews are done (compose view)', async () => {
    // Bug: dismissing the last review on desktop closed the thread drawer
    // because handleDismissThread called navigateToPane('thread'), which
    // unconditionally cleared threadDrawerOpen. The drawer must remain open
    // so the user can pick another thread from the compose view.
    const map = new Map<string, ThreadState>();
    map.set('t1', makeThreadState('t1', {
      meta: { id: 't1', title: 'Last review', channel: 'claude_code', pinned: false, createdAt: '', updatedAt: '2026-01-01T00:00:00Z', unread: true, status: 'waiting', ccHasChanges: true, ccRequiresRestart: false, ccIsExternalRepo: false, ccApplying: false, lastRevivedAt: '', messageCount: 1, section: 'unread', activeChildrenCount: 0 },
    }));
    threadMap.value = map;
    focusThread('t1');
    threadDrawerOpen.value = true;

    await handleDismissThread('t1');

    expect(focusedThreadId.value).toBeNull();
    expect(threadDrawerOpen.value).toBe(true);
  });

  it('focuses the thread above when dismissing the last item', async () => {
    // 2 review threads: t1 (top), t2 (bottom). Dismissing t2 should focus t1.
    const map = new Map<string, ThreadState>();
    map.set('t1', makeThreadState('t1', {
      meta: { id: 't1', title: 'Thread t1', channel: 'claude_code', pinned: false, createdAt: '', updatedAt: '2026-01-01T00:00:01Z', unread: true, status: 'waiting', ccHasChanges: false, ccRequiresRestart: false, ccIsExternalRepo: false, ccApplying: false, lastRevivedAt: '', messageCount: 1, section: 'unread', activeChildrenCount: 0 },
    }));
    map.set('t2', makeThreadState('t2', {
      meta: { id: 't2', title: 'Thread t2', channel: 'claude_code', pinned: false, createdAt: '', updatedAt: '2026-01-01T00:00:00Z', unread: true, status: 'waiting', ccHasChanges: false, ccRequiresRestart: false, ccIsExternalRepo: false, ccApplying: false, lastRevivedAt: '', messageCount: 1, section: 'unread', activeChildrenCount: 0 },
    }));
    threadMap.value = map;
    focusThread('t2');

    await handleDismissThread('t2');

    expect(focusedThreadId.value).toBe('t1');
  });
});
