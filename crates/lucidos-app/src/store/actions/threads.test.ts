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
import { _resetComposeDraftsForTesting, getDraft, setDraft, type ComposeDraft } from '../composeDrafts';
import {
  focusThread,
  unfocusThread,
  handleSaveThread,
  handleArchiveThread,
  handleCloseFocusedThread,
} from './threads';
import { scrolledUp, notAtTop, getResizeMode } from '../../components/chat/scrollState';
import { threadScrollKey } from '../../hooks/useScrollMemory';
import { drawerOpen } from '../../components/layout/Drawer';

import { loadAllThreads, ensureThreadInMap, ensureThreadByIdInMap, upsertThread } from './thread-loading';
import { threadsLoaded, generatedTitleIds } from '../store';

// Mock the API module
vi.mock('../../api/threads', () => ({
  fetchThreads: vi.fn(),
  fetchThreadEvents: vi.fn().mockResolvedValue({ events: [], currentAggregate: null }),
  fetchThreadMessages: vi.fn(),
  saveThread: vi.fn().mockResolvedValue(undefined),
  archiveThread: vi.fn().mockResolvedValue(undefined),
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
  cancelClaudeCode: vi.fn(),
  putComposeOnThread: vi.fn().mockResolvedValue(undefined),
  ensureThreadStarted: vi.fn().mockResolvedValue(undefined),
  deleteThread: vi.fn().mockResolvedValue(undefined),
}));

import { fetchThreads, archiveThread } from '../../api/threads';
import { deleteThread } from '../../api/client';
import { focusPromptNow } from '../../components/chat/promptFocus';

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

interface MakeThreadOverrides extends Partial<Omit<ThreadState, 'meta'>> {
  meta?: Partial<ThreadMeta> & {
    composeText?: string;
    composeImages?: string[];
    composeMode?: ComposeDraft['mode'];
  };
}

function makeThreadState(id: string, overrides: MakeThreadOverrides = {}): ThreadState {
  const { composeText, composeImages, composeMode, ...metaOverrides } = overrides.meta ?? {};
  if (composeText !== undefined || composeImages !== undefined || composeMode !== undefined) {
    setDraft(id, {
      text: composeText ?? '',
      image_hashes: composeImages ?? [],
      mode: composeMode ?? null,
    });
  }
  return {
    meta: {
      id,
      title: `Thread ${id}`,
      channel: 'chat',
      initiator: 'user',
      saved: false,
      createdAt: '2026-01-01T00:00:00Z',
      updatedAt: '2026-01-01T00:00:00Z',
      status: 'idle',
      ccHasChanges: false,
      ccRequiresRestart: false,
      ccIsExternalRepo: false,
      ccApplying: false,
      lastRevivedAt: '',
      messageCount: 0,
      section: 'archived',
      activeChildrenCount: 0,
      totalChildrenCount: 0,
      state: 'active',
      ...metaOverrides,
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
  _resetComposeDraftsForTesting();
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

  it('skips scroll-to-bottom when target thread has a saved scroll position', () => {
    // scrollToBottom's first action is `scrolledUp.value = false` —
    // an unchanged `true` proves it wasn't called.
    const key = threadScrollKey('tSaved');
    try {
      localStorage.setItem(key, '500');
      scrolledUp.value = true;
      focusThread('tSaved');
      expect(scrolledUp.value).toBe(true);
    } finally {
      localStorage.removeItem(key);
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

  it('skipPaneNav keeps the user on the threads pane on mobile', () => {
    // History chevrons in the threads-list header walk the nav stack while
    // keeping the user on the list — they preview where they've been instead
    // of jumping into the thread chat view.
    const origWidth = globalThis.innerWidth;
    (globalThis as any).innerWidth = 375;
    try {
      mobileView.value = 'threads';
      focusThread('t1', { skipPaneNav: true });
      expect(focusedThreadId.value).toBe('t1');
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
// handleSaveThread
// ---------------------------------------------------------------------------

describe('handleSaveThread', () => {
  it('sets saved to true in threadMap', async () => {
    const map = new Map<string, ThreadState>();
    map.set('t1', makeThreadState('t1'));
    threadMap.value = map;

    await handleSaveThread('t1');

    expect(threadMap.value.get('t1')!.meta.saved).toBe(true);
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
      saved: [],
      active_threads: [],
      composing: [],
      active: [],
      history: threads.map(t => ({
        ...t,
        created_at: t.last_activity,
        message_count: 1,
        section: 'archived',
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
      meta: { id: 't1', title: '...', channel: 'chat', saved: false, createdAt: '', updatedAt: '2026-03-15T17:00:00Z', status: 'idle', ccHasChanges: false, ccRequiresRestart: false, ccIsExternalRepo: false, ccApplying: false, lastRevivedAt: '', messageCount: 0, section: 'archived', activeChildrenCount: 0 },
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
      meta: { id: 't1', title: 'Generated Title', channel: 'chat', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:00Z', status: 'idle', ccHasChanges: false, ccRequiresRestart: false, ccIsExternalRepo: false, ccApplying: false, lastRevivedAt: '', messageCount: 0, section: 'archived', activeChildrenCount: 0 },
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
      meta: { id: 't1', title: 'Good Title From SSE', channel: 'chat', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:00Z', status: 'idle', ccHasChanges: false, ccRequiresRestart: false, ccIsExternalRepo: false, ccApplying: false, lastRevivedAt: '', messageCount: 0, section: 'archived', activeChildrenCount: 0 },
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
      meta: { id: 't1', title: '...', channel: 'chat', saved: false, createdAt: '', updatedAt: '2026-03-15T19:30:00Z', status: 'idle', ccHasChanges: false, ccRequiresRestart: false, ccIsExternalRepo: false, ccApplying: false, lastRevivedAt: '', messageCount: 0, section: 'archived', activeChildrenCount: 0 },
    }));
    map.set('t2', makeThreadState('t2', {
      meta: { id: 't2', title: '...', channel: 'chat', saved: false, createdAt: '', updatedAt: '2026-03-15T15:00:00Z', status: 'idle', ccHasChanges: false, ccRequiresRestart: false, ccIsExternalRepo: false, ccApplying: false, lastRevivedAt: '', messageCount: 0, section: 'archived', activeChildrenCount: 0 },
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
      saved: [],
      active_threads: [],
      composing: [],
      active: [],
      history: [
        { thread_id: 't1', title: 'Thread 1', channel: 'chat', last_activity: '2026-03-15T18:00:00Z', created_at: '2026-03-15T18:00:00Z', message_count: 5, section: 'archived', status: 'idle', cc_has_changes: false, cc_requires_restart: false, cc_is_external_repo: false, cc_applying: false, last_revived_at: null, active_children_count: 0 },
        { thread_id: 't2', title: 'Thread 2', channel: 'claude_code', last_activity: '2026-03-15T17:00:00Z', created_at: '2026-03-15T17:00:00Z', message_count: 12, section: 'archived', status: 'idle', cc_has_changes: false, cc_requires_restart: false, cc_is_external_repo: false, cc_applying: false, last_revived_at: null, active_children_count: 0 },
      ],
    });

    await loadAllThreads();

    expect(threadMap.value.get('t1')!.meta.messageCount).toBe(5);
    expect(threadMap.value.get('t2')!.meta.messageCount).toBe(12);
  });

  it('updates messageCount on existing threads from API', async () => {
    const map = new Map<string, ThreadState>();
    map.set('t1', makeThreadState('t1', {
      meta: { id: 't1', title: '...', channel: 'chat', saved: false, createdAt: '', updatedAt: '2026-03-15T19:00:00Z', status: 'idle', ccHasChanges: false, ccRequiresRestart: false, ccIsExternalRepo: false, ccApplying: false, lastRevivedAt: '', messageCount: 0, section: 'archived', activeChildrenCount: 0 },
    }));
    threadMap.value = map;

    (fetchThreads as any).mockResolvedValue({
      saved: [],
      active_threads: [],
      composing: [],
      active: [],
      history: [
        { thread_id: 't1', title: 'Thread 1', channel: 'chat', last_activity: '2026-03-15T18:00:00Z', created_at: '2026-03-15T18:00:00Z', message_count: 7, section: 'archived', status: 'idle', cc_has_changes: false, cc_requires_restart: false, cc_is_external_repo: false, cc_applying: false, last_revived_at: null, active_children_count: 0 },
      ],
    });

    await loadAllThreads();

    expect(threadMap.value.get('t1')!.meta.messageCount).toBe(7);
  });

  it('marks active threads from API active set', async () => {
    (fetchThreads as any).mockResolvedValue({
      saved: [],
      active_threads: [
        { thread_id: 't1', title: 'Active', channel: 'chat', last_activity: '2026-03-15T18:00:00Z', created_at: '2026-03-15T18:00:00Z', message_count: 1, section: 'archived', active_children_count: 0, status: 'running', cc_has_changes: false, cc_requires_restart: false, cc_is_external_repo: false, cc_applying: false, last_revived_at: null },
      ],
      active: ['t1'],
      history: [
        { thread_id: 't2', title: 'Idle', channel: 'chat', last_activity: '2026-03-15T17:00:00Z', created_at: '2026-03-15T17:00:00Z', message_count: 3, section: 'archived', active_children_count: 0, status: 'idle', cc_has_changes: false, cc_requires_restart: false, cc_is_external_repo: false, cc_applying: false, last_revived_at: null },
      ],
      composing: [],
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
      saved: [],
      active_threads: [
        { thread_id: 't1', title: 'Trigger Run', channel: 'trigger', last_activity: '2026-03-15T18:00:00Z', created_at: '2026-03-15T18:00:00Z', message_count: 1, section: 'archived', status: 'running', cc_has_changes: false, cc_requires_restart: false, cc_is_external_repo: false, cc_applying: false, last_revived_at: null, active_children_count: 0 },
      ],
      active: ['t1'],
      history: [],
      composing: [],
    });

    // Event has a different channel — this must NOT overwrite the API channel
    const { fetchThreadEvents } = await import('../../api/threads');
    (fetchThreadEvents as any).mockResolvedValue({ events: [
      {
        sequence: 1,
        event_type: 'MessageReceived',
        payload: { text: 'follow-up', channel: 'chat' },
        created: '2026-03-15T18:00:00Z',
        event_id: 'e1',
      },
    ], currentAggregate: null });

    await loadAllThreads();

    // API channel is preserved — events don't overwrite on initial load
    const thread = threadMap.value.get('t1')!;
    expect(thread.meta.channel).toBe('trigger');
  });
});

// ---------------------------------------------------------------------------
// loadAllThreads — compose-state preservation while user is typing
//
// loadAllThreads runs on SSE reconnect / Lagged events / resume. The API
// returns the LAST PERSISTED compose state, which can be 250ms+ stale due
// to the debounced PUT. Without a guard, upsertThread overwrites the user's
// in-flight keystrokes, losing text and jumping the cursor.
// ---------------------------------------------------------------------------

describe('loadAllThreads — compose preservation', () => {
  beforeEach(() => {
    threadsLoaded.value = false;
  });

  function mockComposeApiResponse(threadId: string, composeText: string, composeImages: string[] = []) {
    (fetchThreads as any).mockResolvedValue({
      saved: [],
      active_threads: [],
      composing: [],
      active: [],
      history: [{
        thread_id: threadId,
        title: 'T',
        channel: 'chat',
        last_activity: '2026-03-15T18:00:00Z',
        created_at: '2026-03-15T18:00:00Z',
        message_count: 0,
        section: 'archived',
        status: 'idle',
        cc_has_changes: false,
        cc_requires_restart: false,
        cc_is_external_repo: false,
        cc_applying: false,
        last_revived_at: null,
        active_children_count: 0,
        state: 'composing',
        compose_text: composeText,
        compose_images: composeImages,
        compose_mode: null,
      }],
    });
  }

  /** Plant a fake textarea matching `[data-role="prompt-input"]` and mark it as
   *  document.activeElement so isComposeFocusedHere() returns true. */
  function focusPromptOnThread(threadId: string): void {
    const el: any = {
      dataset: { threadId, role: 'prompt-input' },
      getBoundingClientRect: () => ({ width: 200, height: 30 }),
    };
    el.getAttribute = (name: string) => name === 'data-role' ? 'prompt-input' : el.dataset[name];
    (globalThis.document as any).querySelectorAll = (sel: string) => sel === '[data-role="prompt-input"]' ? [el] : [];
    (globalThis.document as any).activeElement = el;
  }

  function unfocusPrompt(): void {
    (globalThis.document as any).querySelectorAll = () => [];
    (globalThis.document as any).activeElement = null;
  }

  it('preserves local composeText when textarea is focused on this thread', async () => {
    const map = new Map<string, ThreadState>();
    map.set('t1', makeThreadState('t1', { meta: { state: 'composing', composeText: 'hello world I am typing' } }));
    threadMap.value = map;
    focusedThreadId.value = 't1';
    focusPromptOnThread('t1');

    // API returns stale compose_text from before the user's keystrokes
    mockComposeApiResponse('t1', 'hello');

    await loadAllThreads();

    expect(getDraft('t1').text).toBe('hello world I am typing');
    unfocusPrompt();
  });

  it('preserves local composeImages when textarea is focused on this thread', async () => {
    const map = new Map<string, ThreadState>();
    map.set('t1', makeThreadState('t1', { meta: { state: 'composing', composeImages: ['local-img-base64'] } }));
    threadMap.value = map;
    focusedThreadId.value = 't1';
    focusPromptOnThread('t1');

    mockComposeApiResponse('t1', '', []);

    await loadAllThreads();

    expect(getDraft('t1').image_hashes).toEqual(['local-img-base64']);
    unfocusPrompt();
  });

  it('preserves local composeText when a PUT is in flight (debounced push not yet acked)', async () => {
    const { pendingComposePuts } = await import('./compose');
    const map = new Map<string, ThreadState>();
    map.set('t1', makeThreadState('t1', { meta: { state: 'composing', composeText: 'half-typed sentence' } }));
    threadMap.value = map;
    pendingComposePuts.add('t1');
    unfocusPrompt(); // textarea may have lost focus mid-PUT

    mockComposeApiResponse('t1', 'half'); // server has older state

    try {
      await loadAllThreads();
      expect(getDraft('t1').text).toBe('half-typed sentence');
    } finally {
      pendingComposePuts.delete('t1');
    }
  });

  it('does refresh composeText when no local edit is in flight (cross-device sync still works)', async () => {
    const map = new Map<string, ThreadState>();
    map.set('t1', makeThreadState('t1', { meta: { state: 'composing', composeText: 'old local' } }));
    threadMap.value = map;
    unfocusPrompt(); // not actively typing here
    // pendingComposePuts is empty

    mockComposeApiResponse('t1', 'updated by peer');

    await loadAllThreads();

    expect(getDraft('t1').text).toBe('updated by peer');
  });

  // iOS PWA photo-attach regression: PHPicker dismissal fires visibilitychange
  // (which kicks off loadAllThreads) right around the same instant as the file
  // input's change event. The change handler resolves a FileReader and calls
  // updateCompose, which only commits the optimistic image to threadMap and
  // schedules the PUT for 250ms later. If loadAllThreads lands inside that
  // debounce window, the textarea isn't focused (the picker stole it) and
  // pendingComposePuts is still empty, so upsertThread overwrites the freshly
  // attached image with the server's stale empty array — preview never appears.
  it('preserves locally-attached image when loadAllThreads lands inside the PUT debounce window', async () => {
    const { updateCompose, pendingComposePuts, composeEditedAt } = await import('./compose');
    const map = new Map<string, ThreadState>();
    map.set('t1', makeThreadState('t1', { meta: { state: 'active', composeText: '', composeImages: [] } }));
    threadMap.value = map;
    focusedThreadId.value = 't1';
    unfocusPrompt(); // PHPicker took focus away from the textarea

    // User just attached an image — optimistic update committed, PUT debounced
    updateCompose('t1', { image_hashes: ['attached-img-base64'] });

    // Server's last persisted state is still empty (PUT hasn't been sent yet)
    mockComposeApiResponse('t1', '', []);

    try {
      await loadAllThreads();
      expect(getDraft('t1').image_hashes).toEqual(['attached-img-base64']);
    } finally {
      // Clean up the entries the real updateCompose put there. The 250ms timer
      // it also scheduled will still fire after this test, but pushNow's
      // missing-thread guard will see threadMap empty and bail without an HTTP
      // call — so the leak is bounded to the entries below.
      composeEditedAt.delete('t1');
      pendingComposePuts.delete('t1');
    }
  });

  // "Preview appears then disappears" — the user reports this on macOS Chrome
  // and iOS Safari PWA. The previous fix covers the case where loadAllThreads
  // lands DURING the debounce window. This test covers the harder case: the
  // GET was sent BEFORE the optimistic write (so server's snapshot has empty
  // images), but its RESPONSE arrives AFTER pushNow's PUT has completed and
  // pendingComposePuts has been cleared. The guard sees no pending entry and
  // overwrites the freshly attached image with the stale server snapshot.
  it('preserves locally-attached image when stale loadAllThreads response lands after PUT completes', async () => {
    const { updateCompose, pendingComposePuts, composeEditedAt } = await import('./compose');

    const map = new Map<string, ThreadState>();
    map.set('t1', makeThreadState('t1', { meta: { state: 'active', composeText: '', composeImages: [] } }));
    threadMap.value = map;
    focusedThreadId.value = 't1';
    unfocusPrompt(); // textarea isn't focused (picker stole focus)

    // Server's snapshot is from BEFORE the user's PUT. fetchThreads is mocked
    // to defer until we resolve the gate, simulating a slow GET.
    let releaseFetch: (() => void) | null = null;
    const fetchGate = new Promise<void>((resolve) => { releaseFetch = resolve; });
    (fetchThreads as any).mockImplementationOnce(async () => {
      await fetchGate;
      return {
        saved: [],
        active_threads: [],
        composing: [],
        active: [],
        history: [{
          thread_id: 't1',
          title: 'T',
          channel: 'chat',
          last_activity: '2026-03-15T18:00:00Z',
          created_at: '2026-03-15T18:00:00Z',
          message_count: 0,
          section: 'archived',
          status: 'idle',
          cc_has_changes: false,
          cc_requires_restart: false,
          cc_is_external_repo: false,
          cc_applying: false,
          last_revived_at: null,
          active_children_count: 0,
          state: 'active',
          compose_text: '',
          compose_images: [], // STALE — pre-PUT snapshot
          compose_mode: null,
        }],
      };
    });

    try {
      // Kick off loadAllThreads. Its HTTP is in flight.
      const loadPromise = loadAllThreads();

      // User attaches an image. updateCompose: optimistic + pending mark + 250ms timer.
      updateCompose('t1', { image_hashes: ['attached-img-base64'] });
      expect(pendingComposePuts.has('t1')).toBe(true);

      // Wait for the 250ms debounce + pushNow's await to drain. After this,
      // pendingComposePuts is cleared (PUT completed).
      await new Promise((r) => setTimeout(r, 300));
      expect(pendingComposePuts.has('t1')).toBe(false);

      // NOW the slow loadAllThreads response arrives with stale empty images.
      releaseFetch!();
      await loadPromise;

      // Bug: stale loadAllThreads overwrites composeImages with []. Preview disappears.
      expect(getDraft('t1').image_hashes).toEqual(['attached-img-base64']);
    } finally {
      // updateCompose stamps composeEditedAt; clear so the next test using 't1'
      // sees a clean slate (the 'cross-device sync still works' test would
      // otherwise see this thread as still-locally-edited and skip the apply).
      composeEditedAt.delete('t1');
      pendingComposePuts.delete('t1');
    }
  });
});

// ---------------------------------------------------------------------------
// ensureThreadInMap — search result click bootstrapping
// ---------------------------------------------------------------------------

describe('ensureThreadInMap', () => {
  it('creates a skeleton ThreadState for a thread not in the map', async () => {
    threadsLoaded.value = true;

    const { fetchThreadEvents } = await import('../../api/threads');
    (fetchThreadEvents as any).mockResolvedValue({ events: [
      {
        sequence: 1,
        event_type: 'MessageReceived',
        payload: { text: 'hello world', channel: 'chat' },
        created: '2026-03-15T18:00:00Z',
        event_id: 'e1',
      },
    ], currentAggregate: null });

    expect(threadMap.value.has('search-t1')).toBe(false);

    await ensureThreadInMap({
      thread_id: 'search-t1',
      title: 'Search Result Thread',
      channel: 'chat',
      initiator: 'user',
      last_activity: '2026-03-15T18:00:00Z',
      created_at: '2026-03-15T18:00:00Z',
      message_count: 3,
      section: 'archived',
      active_children_count: 0,
      total_children_count: 0,
      status: 'idle',
      cc_has_changes: false,
      cc_requires_restart: false,
      cc_is_external_repo: false,
      cc_applying: false, last_revived_at: null,
      state: 'active',
      compose_text: '',
      compose_images: [],
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
      meta: { id: 't1', title: 'Existing Title', channel: 'claude_code', initiator: 'user', saved: true, createdAt: '', updatedAt: '2026-01-01T00:00:00Z', status: 'idle', ccHasChanges: false, ccRequiresRestart: false, ccIsExternalRepo: false, ccApplying: false, lastRevivedAt: '', messageCount: 5, section: 'archived', activeChildrenCount: 0 },
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
      section: 'archived',
      active_children_count: 0,
      total_children_count: 0,
      status: 'idle',
      cc_has_changes: false,
      cc_requires_restart: false,
      cc_is_external_repo: false,
      cc_applying: false, last_revived_at: null,
      state: 'active',
      compose_text: '',
      compose_images: [],
    });

    const thread = threadMap.value.get('t1')!;
    expect(thread.meta.title).toBe('Existing Title');
    expect(thread.meta.saved).toBe(true);
  });
});

// ---------------------------------------------------------------------------
// ensureThreadByIdInMap — thread-link click bootstrapping when only the ID
// is known (the thread isn't in the loaded list — e.g. an archived thread
// past the per-source History window).
// ---------------------------------------------------------------------------

describe('ensureThreadByIdInMap', () => {
  it('fetches metadata for a thread not in the map and adds it', async () => {
    threadsLoaded.value = true;
    threadMap.value = new Map();

    (fetchThreads as any).mockResolvedValue({
      saved: [],
      history: [],
      active: [],
      active_threads: [],
      composing: [],
      focused_thread: {
        thread_id: 'old-archived-1',
        title: 'Old Archived Thread',
        channel: 'chat',
        initiator: 'user',
        last_activity: '2026-01-01T00:00:00Z',
        created_at: '2026-01-01T00:00:00Z',
        message_count: 4,
        section: 'archived',
        active_children_count: 0,
        total_children_count: 0,
        status: 'idle',
        cc_has_changes: false,
        cc_requires_restart: false,
        cc_is_external_repo: false,
        cc_applying: false,
        last_revived_at: null,
      },
    });

    const ok = await ensureThreadByIdInMap('old-archived-1');
    expect(ok).toBe(true);
    expect(fetchThreads).toHaveBeenCalledWith('old-archived-1');
    const added = threadMap.value.get('old-archived-1');
    expect(added).toBeDefined();
    expect(added!.meta.title).toBe('Old Archived Thread');
    expect(added!.meta.channel).toBe('chat');
  });

  it('returns true without fetching when the thread is already in the map', async () => {
    const map = new Map<string, ThreadState>();
    map.set('already-here', makeThreadState('already-here', {
      meta: { id: 'already-here', title: 'Already Here', channel: 'chat', initiator: 'user', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:00Z', status: 'idle', ccHasChanges: false, ccRequiresRestart: false, ccIsExternalRepo: false, ccApplying: false, lastRevivedAt: '', messageCount: 1, section: 'inbox', activeChildrenCount: 0 },
    }));
    threadMap.value = map;
    (fetchThreads as any).mockClear();

    const ok = await ensureThreadByIdInMap('already-here');
    expect(ok).toBe(true);
    expect(fetchThreads).not.toHaveBeenCalled();
  });

  it('returns false when the API has no record of the thread', async () => {
    threadMap.value = new Map();
    (fetchThreads as any).mockResolvedValue({
      saved: [], history: [], active: [], active_threads: [], composing: [],
      // no focused_thread → API doesn't know this thread
    });

    const ok = await ensureThreadByIdInMap('does-not-exist');
    expect(ok).toBe(false);
    expect(threadMap.value.has('does-not-exist')).toBe(false);
  });

  it('propagates fetch errors to the caller (no swallow)', async () => {
    threadMap.value = new Map();
    const apiError = new Error('network down');
    (fetchThreads as any).mockRejectedValue(apiError);

    await expect(ensureThreadByIdInMap('any-id')).rejects.toThrow('network down');
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
        saved: false,
        createdAt: '2026-03-19T20:00:00Z',
        updatedAt: '2026-03-19T20:00:00Z',
        status: 'running',
      ccHasChanges: false,
      ccRequiresRestart: false,
      ccIsExternalRepo: false,
      ccApplying: false,  // Set by SSE skeleton
        lastRevivedAt: '',
        messageCount: 0,
        section: 'archived',
        activeChildrenCount: 0,
      },
    }));
    threadMap.value = map;

    // API response includes the CC thread in history but NOT in active set
    // (because the CC session hasn't registered itself yet)
    (fetchThreads as any).mockResolvedValue({
      saved: [],
      active_threads: [],
      composing: [],
      active: [],  // CC thread NOT in active set
      history: [
        { thread_id: 'cc-1', title: 'Fix OAuth URLs', channel: 'claude_code', last_activity: '2026-03-19T20:00:00Z', created_at: '2026-03-19T20:00:00Z', message_count: 1, section: 'archived', active_children_count: 0, status: 'idle', cc_has_changes: false, cc_requires_restart: false, cc_is_external_repo: false, cc_applying: false, last_revived_at: null },
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
        saved: false,
        createdAt: '2026-03-19T20:00:00Z',
        updatedAt: '2026-03-19T20:00:00Z',
        status: 'running',
      ccHasChanges: false,
      ccRequiresRestart: false,
      ccIsExternalRepo: false,
      ccApplying: false,  // Was running before restart
        lastRevivedAt: '',
        messageCount: 0,
        section: 'archived',
        activeChildrenCount: 0,
      },
      eventsLoaded: true,  // Events have been loaded — downgrade is safe
    }));
    threadMap.value = map;

    (fetchThreads as any).mockResolvedValue({
      saved: [],
      active_threads: [],
      composing: [],
      active: [],  // Thread no longer active after restart
      history: [
        { thread_id: 'cc-1', title: 'Aborted Session', channel: 'claude_code', last_activity: '2026-03-19T20:00:00Z', created_at: '2026-03-19T20:00:00Z', message_count: 1, section: 'inbox', status: 'idle', cc_has_changes: false, cc_requires_restart: false, cc_is_external_repo: false, cc_applying: false, last_revived_at: null, active_children_count: 0 },
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
      saved: [],
      active_threads: [
        {
          thread_id: 'stuck-t1',
          title: 'Stuck Thread',
          channel: 'claude_code',
          last_activity: '2026-03-30T09:00:00Z',
          created_at: '2026-03-28T19:00:00Z',
          message_count: 5,
          section: 'archived',
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
      composing: [],
    });

    const { fetchThreadEvents } = await import('../../api/threads');
    (fetchThreadEvents as any).mockResolvedValue({ events: [
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
    ], currentAggregate: {
      // Backend snapshot: status='idle' (last revival is in the past, agent dead).
      // currentAggregate is the source of truth — frontend overlays it after
      // replaying events, preventing per-event derivations from leaking through.
      threadId: 'stuck-t1',
      title: 'Stuck Thread',
      channel: 'claude_code',
      initiator: 'user',
      createdAt: '2026-03-28T19:00:00Z',
      lastActivity: '2026-03-30T08:41:01Z',
      messageCount: 5,
      section: 'inbox',
      status: 'idle',
      activeChildrenCount: 0,
      totalChildrenCount: 0,
      ccHasChanges: false,
      ccRequiresRestart: false,
      ccIsExternalRepo: false,
      ccApplying: false,
      isSaved: false,
      hasResponse: true,
      lastRevivedAt: null,
      parentThreadId: null,
      parentThreadTitle: null,
      state: 'active',
    } });

    await loadAllThreads();

    const thread = threadMap.value.get('stuck-t1')!;
    // currentAggregate said 'idle' — replay must NOT override this to 'running'
    expect(thread.meta.status).toBe('idle');
  });

  it('refreshThreadEvents allows new events to update status', async () => {
    // Scenario: thread was idle, user sends a message while SSE was disconnected.
    // On reconnect, refreshThreadEvents fetches the new MessageReceived from DB.
    // The event must update status to 'running' — it's a real live event, not stale replay.
    const map = new Map<string, ThreadState>();
    map.set('refresh-t1', makeThreadState('refresh-t1', {
      meta: { id: 'refresh-t1', title: 'Refresh Thread', channel: 'claude_code', saved: false, createdAt: '2026-03-28T19:00:00Z', updatedAt: '2026-03-28T19:00:00Z', status: 'idle', ccHasChanges: false, ccRequiresRestart: false, ccIsExternalRepo: false, ccApplying: false, lastRevivedAt: '', messageCount: 1, section: 'archived', activeChildrenCount: 0 },
      eventsLoaded: true,
      lastDbSeq: 3,
    }));
    threadMap.value = map;

    const { fetchThreadEvents } = await import('../../api/threads');
    (fetchThreadEvents as any).mockResolvedValue({ events: [
      {
        sequence: 4,
        event_type: 'MessageReceived',
        payload: { text: 'new message', channel: 'claude_code' },
        created: '2026-03-30T10:00:00Z',
        event_id: 'e4',
      },
    ], currentAggregate: {
      // Backend snapshot after the new MessageReceived: status='running'.
      threadId: 'refresh-t1',
      title: 'Refresh Thread',
      channel: 'claude_code',
      initiator: 'user',
      createdAt: '2026-03-28T19:00:00Z',
      lastActivity: '2026-03-30T10:00:00Z',
      messageCount: 2,
      section: 'archived',
      status: 'running',
      activeChildrenCount: 0,
      totalChildrenCount: 0,
      ccHasChanges: false,
      ccRequiresRestart: false,
      ccIsExternalRepo: false,
      ccApplying: false,
      isSaved: false,
      hasResponse: false,
      lastRevivedAt: '2026-03-30T10:00:00Z',
      parentThreadId: null,
      parentThreadTitle: null,
      state: 'active',
    } });

    const { refreshThreadEvents } = await import('./thread-loading');
    await refreshThreadEvents('refresh-t1');

    // New event must update status — refreshThreadEvents does NOT preserve stale status
    expect(threadMap.value.get('refresh-t1')!.meta.status).toBe('running');
  });
});

// ---------------------------------------------------------------------------
// Section — focusThread does NOT auto-archive (user must click Archive/Apply/Discard)
// ---------------------------------------------------------------------------

describe('focusThread — section', () => {
  it('does not archive an inbox thread when focused', () => {
    const map = new Map<string, ThreadState>();
    map.set('t1', makeThreadState('t1', {
      meta: { id: 't1', title: 'Inbox Thread', channel: 'chat', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:00Z', status: 'idle', ccHasChanges: false, ccRequiresRestart: false, ccIsExternalRepo: false, ccApplying: false, lastRevivedAt: '', messageCount: 0, section: 'inbox', activeChildrenCount: 0 },
    }));
    threadMap.value = map;

    focusThread('t1');

    // Section stays 'inbox' — no auto-archive on focus
    expect(threadMap.value.get('t1')!.meta.section).toBe('inbox');
  });

  it('does not update threadMap when section is already archived', () => {
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
      saved: [],
      active_threads: [],
      composing: [],
      active: [],
      history: [
        { thread_id: 't1', title: 'Unread', channel: 'chat', last_activity: '2026-03-15T18:00:00Z', created_at: '2026-03-15T18:00:00Z', message_count: 1, section: 'inbox' },
        { thread_id: 't2', title: 'Normal', channel: 'chat', last_activity: '2026-03-15T17:00:00Z', created_at: '2026-03-15T17:00:00Z', message_count: 2, section: 'archived' },
      ],
    });

    await loadAllThreads();

    expect(threadMap.value.get('t1')!.meta.section).toBe('inbox');
    expect(threadMap.value.get('t2')!.meta.section).toBe('archived');
  });

  it('updates section on existing threads from API', async () => {
    const map = new Map<string, ThreadState>();
    map.set('t1', makeThreadState('t1', {
      meta: { id: 't1', title: '...', channel: 'chat', saved: false, createdAt: '', updatedAt: '2026-03-15T19:00:00Z', status: 'idle', ccHasChanges: false, ccRequiresRestart: false, ccIsExternalRepo: false, ccApplying: false, lastRevivedAt: '', messageCount: 0, section: 'archived', activeChildrenCount: 0 },
    }));
    threadMap.value = map;

    (fetchThreads as any).mockResolvedValue({
      saved: [],
      active_threads: [],
      composing: [],
      active: [],
      history: [
        { thread_id: 't1', title: 'Thread 1', channel: 'chat', last_activity: '2026-03-15T18:00:00Z', created_at: '2026-03-15T18:00:00Z', message_count: 1, section: 'inbox' },
      ],
    });

    await loadAllThreads();

    expect(threadMap.value.get('t1')!.meta.section).toBe('inbox');
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
        id: 't1', title: 'CC Thread', channel: 'claude_code', saved: false,
        createdAt: '2026-03-31T19:21:38Z',
        updatedAt: '2026-03-31T19:31:54Z', // SSE-updated (newer)
        status: 'running', ccHasChanges: false,
        ccRequiresRestart: false, ccIsExternalRepo: false, ccApplying: false, lastRevivedAt: '',
        messageCount: 0, section: 'archived', activeChildrenCount: 0,
      },
    }));

    upsertThread(map, {
      thread_id: 't1',
      title: 'CC Thread',
      channel: 'claude_code',
      last_activity: '2026-03-31T19:21:38Z', // stale backend value
      created_at: '2026-03-31T19:21:38Z',
      message_count: 1,
      section: 'archived',
      status: 'running',
    } as any, false);

    // updatedAt must keep the newer SSE value, not regress to the stale API value
    expect(map.get('t1')!.meta.updatedAt).toBe('2026-03-31T19:31:54Z');
  });

  it('advances updatedAt when API returns a newer last_activity', () => {
    const map = new Map<string, ThreadState>();
    map.set('t1', makeThreadState('t1', {
      meta: {
        id: 't1', title: 'Thread', channel: 'chat', saved: false,
        createdAt: '2026-03-31T10:00:00Z',
        updatedAt: '2026-03-31T10:00:00Z',
        status: 'idle', ccHasChanges: false,
        ccRequiresRestart: false, ccIsExternalRepo: false, ccApplying: false, lastRevivedAt: '',
        messageCount: 0, section: 'archived', activeChildrenCount: 0,
      },
    }));

    upsertThread(map, {
      thread_id: 't1',
      title: 'Thread',
      channel: 'chat',
      last_activity: '2026-03-31T12:00:00Z', // newer
      created_at: '2026-03-31T10:00:00Z',
      message_count: 2,
      section: 'archived',
      status: 'idle',
    } as any, false);

    // API has a newer timestamp — should advance
    expect(map.get('t1')!.meta.updatedAt).toBe('2026-03-31T12:00:00Z');
  });
});

// ---------------------------------------------------------------------------
// handleArchiveThread — focus next review thread
// ---------------------------------------------------------------------------

describe('handleArchiveThread', () => {
  it('focuses the next review thread after dismissing', async () => {
    // Set up: t1 (focused, waiting/inbox = review), t2 (also waiting/inbox = review)
    const map = new Map<string, ThreadState>();
    map.set('t1', makeThreadState('t1', {
      meta: { id: 't1', title: 'Thread t1', channel: 'claude_code', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:01Z', status: 'waiting', ccHasChanges: true, ccRequiresRestart: false, ccIsExternalRepo: false, ccApplying: false, lastRevivedAt: '', messageCount: 1, section: 'inbox', activeChildrenCount: 0 },
    }));
    map.set('t2', makeThreadState('t2', {
      meta: { id: 't2', title: 'Thread t2', channel: 'claude_code', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:00Z', status: 'waiting', ccHasChanges: true, ccRequiresRestart: false, ccIsExternalRepo: false, ccApplying: false, lastRevivedAt: '', messageCount: 1, section: 'inbox', activeChildrenCount: 0 },
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
      meta: { id: 't1', title: 'Thread t1', channel: 'claude_code', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:00Z', status: 'waiting', ccHasChanges: true, ccRequiresRestart: false, ccIsExternalRepo: false, ccApplying: false, lastRevivedAt: '', messageCount: 1, section: 'inbox', activeChildrenCount: 0 },
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
      meta: { id: 't1', title: 'Last review', channel: 'claude_code', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:00Z', status: 'waiting', ccHasChanges: true, ccRequiresRestart: false, ccIsExternalRepo: false, ccApplying: false, lastRevivedAt: '', messageCount: 1, section: 'inbox', activeChildrenCount: 0 },
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
      meta: { id: 't1', title: 'Thread t1', channel: 'claude_code', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:02Z', status: 'waiting', ccHasChanges: false, ccRequiresRestart: false, ccIsExternalRepo: false, ccApplying: false, lastRevivedAt: '', messageCount: 1, section: 'inbox', activeChildrenCount: 0 },
    }));
    map.set('t2', makeThreadState('t2', {
      meta: { id: 't2', title: 'Thread t2', channel: 'claude_code', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:01Z', status: 'waiting', ccHasChanges: false, ccRequiresRestart: false, ccIsExternalRepo: false, ccApplying: false, lastRevivedAt: '', messageCount: 1, section: 'inbox', activeChildrenCount: 0 },
    }));
    map.set('t3', makeThreadState('t3', {
      meta: { id: 't3', title: 'Thread t3', channel: 'claude_code', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:00Z', status: 'waiting', ccHasChanges: false, ccRequiresRestart: false, ccIsExternalRepo: false, ccApplying: false, lastRevivedAt: '', messageCount: 1, section: 'inbox', activeChildrenCount: 0 },
    }));
    threadMap.value = map;
    focusThread('t2');

    await handleArchiveThread('t2');

    expect(focusedThreadId.value).toBe('t3');
  });

  it('navigates to next review thread after apply+archive (regression: Apply used to skip Archive)', async () => {
    // Simulates the fixed flow: after Apply, thread stays in review (section=inbox,
    // ccHasChanges=false), Archive button appears, clicking Archive navigates to next thread.
    // Previously Apply moved the thread to HISTORY immediately, skipping Archive.
    const map = new Map<string, ThreadState>();
    // t1: just applied — still in review with Archive button (inbox, no changes, idle)
    map.set('t1', makeThreadState('t1', {
      meta: { id: 't1', title: 'Applied thread', channel: 'claude_code', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:01Z', status: 'idle', ccHasChanges: false, ccRequiresRestart: false, ccIsExternalRepo: false, ccApplying: false, lastRevivedAt: '', messageCount: 1, section: 'inbox', activeChildrenCount: 0 },
    }));
    // t2: another review thread waiting for attention
    map.set('t2', makeThreadState('t2', {
      meta: { id: 't2', title: 'Pending thread', channel: 'claude_code', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:00Z', status: 'waiting', ccHasChanges: true, ccRequiresRestart: false, ccIsExternalRepo: false, ccApplying: false, lastRevivedAt: '', messageCount: 1, section: 'inbox', activeChildrenCount: 0 },
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
      meta: { id: 't1', title: 'Last review', channel: 'claude_code', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:00Z', status: 'waiting', ccHasChanges: true, ccRequiresRestart: false, ccIsExternalRepo: false, ccApplying: false, lastRevivedAt: '', messageCount: 1, section: 'inbox', activeChildrenCount: 0 },
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
      meta: { id: 't1', title: 'Thread t1', channel: 'claude_code', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:01Z', status: 'waiting', ccHasChanges: false, ccRequiresRestart: false, ccIsExternalRepo: false, ccApplying: false, lastRevivedAt: '', messageCount: 1, section: 'inbox', activeChildrenCount: 0 },
    }));
    map.set('t2', makeThreadState('t2', {
      meta: { id: 't2', title: 'Thread t2', channel: 'claude_code', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:00Z', status: 'waiting', ccHasChanges: false, ccRequiresRestart: false, ccIsExternalRepo: false, ccApplying: false, lastRevivedAt: '', messageCount: 1, section: 'inbox', activeChildrenCount: 0 },
    }));
    threadMap.value = map;
    focusThread('t2');

    await handleArchiveThread('t2');

    expect(focusedThreadId.value).toBe('t1');
  });
});

// ---------------------------------------------------------------------------
// handleCloseFocusedThread — Cmd/Ctrl+Shift+W shortcut
// ---------------------------------------------------------------------------

describe('handleCloseFocusedThread', () => {
  beforeEach(() => {
    (archiveThread as ReturnType<typeof vi.fn>).mockClear();
    (deleteThread as ReturnType<typeof vi.fn>).mockClear();
  });

  it('no-ops when no thread is focused', async () => {
    focusedThreadId.value = null;

    await handleCloseFocusedThread();

    expect(archiveThread).not.toHaveBeenCalled();
    expect(deleteThread).not.toHaveBeenCalled();
  });

  it('discards a composing draft', async () => {
    const map = new Map<string, ThreadState>();
    map.set('t1', makeThreadState('t1', {
      meta: { state: 'composing', section: 'inbox' },
    }));
    threadMap.value = map;
    focusThread('t1');

    await handleCloseFocusedThread();

    expect(deleteThread).toHaveBeenCalledWith('t1');
    // discardCompose unfocuses the thread on the way out.
    expect(focusedThreadId.value).toBeNull();
  });

  it('archives an active idle inbox thread', async () => {
    const map = new Map<string, ThreadState>();
    map.set('t1', makeThreadState('t1', {
      meta: { id: 't1', channel: 'chat', state: 'active', status: 'idle', section: 'inbox' },
    }));
    threadMap.value = map;
    focusThread('t1');

    await handleCloseFocusedThread();

    expect(archiveThread).toHaveBeenCalledWith('t1');
  });

  it('does not archive a thread that is mid-turn (running)', async () => {
    const map = new Map<string, ThreadState>();
    map.set('t1', makeThreadState('t1', {
      meta: { id: 't1', channel: 'chat', state: 'active', status: 'running', section: 'inbox' },
    }));
    threadMap.value = map;
    focusThread('t1');

    await handleCloseFocusedThread();

    expect(archiveThread).not.toHaveBeenCalled();
  });

  it('does not archive a CC thread with pending changes', async () => {
    // Pending changes need an explicit Apply or Discard — close shortcut must
    // not silently archive over a decision the user hasn't made yet.
    const map = new Map<string, ThreadState>();
    map.set('t1', makeThreadState('t1', {
      meta: { id: 't1', channel: 'claude_code', state: 'active', status: 'waiting', section: 'inbox', ccHasChanges: true },
    }));
    threadMap.value = map;
    focusThread('t1');

    await handleCloseFocusedThread();

    expect(archiveThread).not.toHaveBeenCalled();
  });

  it('does not archive an already-archived thread', async () => {
    const map = new Map<string, ThreadState>();
    map.set('t1', makeThreadState('t1', {
      meta: { id: 't1', channel: 'chat', state: 'active', status: 'idle', section: 'archived' },
    }));
    threadMap.value = map;
    focusThread('t1');

    await handleCloseFocusedThread();

    expect(archiveThread).not.toHaveBeenCalled();
  });
});
