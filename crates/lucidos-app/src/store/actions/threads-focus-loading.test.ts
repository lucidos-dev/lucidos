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
  // setFollowLiveEdge() needs document.querySelector and requestAnimationFrame
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
import { fetchThreads } from '../../api/threads';
import { awayFromBottom, isFollowScroll, notAtTop, setActiveScrollElement, setFollowLiveEdge, stopFollowingBottom } from '../../components/chat/scrollState';
import { drawerOpen } from '../../components/layout/Drawer';
import { threadScrollKey } from '../../hooks/useScrollMemory';
import { _resetComposeDraftsForTesting, getDraft } from '../composeDrafts';
import { archiveThreadCount, archivingThreadIds, codingAgentPendingModel, codingAgentPendingReasoningEffort, focusedPane, focusedThreadId, generatedTitleIds, mobileView, resetCodingAgentPendingPreferences, threadDrawerOpen, threadMap, threadsLoaded } from '../store';
import { loadAllThreads } from './thread-loading';
import { focusThread, handleSaveThread, unfocusThread } from './threads';

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
  composeHandlers: vi.fn(),
}));

vi.mock('../../api/client', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../api/client')>()),
  API_BASE: '',
  submitChat: vi.fn().mockResolvedValue(undefined),
  cancelChat: vi.fn(),
  stopClaudeCode: vi.fn(),
  putComposeOnThread: vi.fn().mockResolvedValue({ status: 'applied' }),
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
  resetCodingAgentPendingPreferences();
  generatedTitleIds.clear();
  archivingThreadIds.value = new Set();
  focusedPane.value = 'thread';
  localStorage.removeItem('lucidos-focused-thread');
});

describe('focusThread', () => {
  it('sets focusedThreadId', () => {
    focusThread('t1');
    expect(focusedThreadId.value).toBe('t1');
  });

  it('does not move the transcript, whatever it finds there', () => {
    // focusThread used to scroll the transcript to the bottom for any thread
    // with no saved position, and skip when there was one. It positions nothing
    // now: `useScrollMemory` owns the opening position on both branches (restore
    // a saved one, else open at the top via `resetOnEmpty`), so focusThread
    // cannot disagree with it about where the reader lands.
    const el = {
      parentElement: null,
      scrollTop: 1234,
      scrollHeight: 9000,
      clientHeight: 500,
      getBoundingClientRect: () => ({ width: 400, height: 500 }),
    } as any;
    setActiveScrollElement(el);
    try {
      focusThread('t1');
      expect(el.scrollTop).toBe(1234);
      expect(focusedThreadId.value).toBe('t1');

      const key = threadScrollKey('tSaved');
      localStorage.setItem(key, '500');
      try {
        focusThread('tSaved');
        expect(el.scrollTop).toBe(1234);
      } finally {
        localStorage.removeItem(key);
      }
    } finally {
      setActiveScrollElement(null);
    }
  });

  // The standing follow is one global, so the thread being OPENED must not
  // inherit the one the reader armed in the thread they left. The request
  // itself is not lost: it is recorded as that thread's reading position and
  // resumed on re-entry (see `hooks/useScrollMemory.ts`).
  describe('the standing follow and the thread being opened', () => {
    /** A transcript whose reader is already AT the live edge, so arming the
     *  follow writes no scroll and runs no tween. These tests are about
     *  `focusThread` retiring the follow, not about how the toggle travels, and
     *  the rAF stub at the top of this file runs its callback synchronously,
     *  which a tween would recurse on forever. */
    function makeTranscript() {
      return {
        parentElement: null,
        scrollTop: 8500, // 9000 - 500, the live edge
        scrollHeight: 9000,
        clientHeight: 500,
        getBoundingClientRect: () => ({ width: 400, height: 500 }),
      } as any;
    }

    function withTranscript(run: (el: any) => void) {
      const el = makeTranscript();
      setActiveScrollElement(el);
      try {
        run(el);
      } finally {
        stopFollowingBottom();
        setActiveScrollElement(null);
      }
    }

    it('retires it when the reader opens a DIFFERENT thread', () => {
      withTranscript((el) => {
        focusThread('t1');
        setFollowLiveEdge(true); // the reader arms it here
        expect(isFollowScroll(el)).toBe(true);

        focusThread('t2');
        expect(isFollowScroll(el)).toBe(false);
      });
    });

    it('keeps it when the reader re-taps the thread they are already in', () => {
      // Not an open at all: nothing is inheriting anything, and the scroll
      // memory does not re-run on an unchanged key, so a retire here would end
      // the follow with nothing left to resume it.
      withTranscript((el) => {
        focusThread('t1');
        setFollowLiveEdge(true);

        focusThread('t1');
        expect(isFollowScroll(el)).toBe(true);
      });
    });

    it('retires it when the reader leaves for the compose view', () => {
      // The compose view has its own scroll container and registers itself as
      // the active one, so a follow left armed would ride its growth instead.
      withTranscript((el) => {
        focusThread('t1');
        setFollowLiveEdge(true);

        unfocusThread();
        expect(isFollowScroll(el)).toBe(false);
      });
    });
  });

  it('leaves the chevron signals to the scroll listener', () => {
    // notAtTop and awayFromBottom must NOT be manually reset here: the scroll
    // listener owns them exclusively. A manual reset makes the chevron vanish
    // when no scroll event follows, e.g. re-focusing the thread you are already
    // in, where scrollTop does not change at all.
    notAtTop.value = true;
    awayFromBottom.value = true;
    focusThread('t1');
    focusThread('t1'); // re-focus the same thread
    expect(notAtTop.value).toBe(true);
    expect(awayFromBottom.value).toBe(true);
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

  // Pane-group activation: navigating to a thread must re-activate the Threads
  // pane group when arriving from the Content group, so keyboard Tab (handlePaneTab,
  // anchored on focusedPane) lands on the conversation rather than the content view.
  it('desktop: re-activates the Threads pane group when arriving from content', () => {
    const origWidth = globalThis.innerWidth;
    (globalThis as any).innerWidth = 1024; // desktop
    try {
      focusedPane.value = 'content'; // user was viewing an app/Settings
      focusThread('t1');
      expect(focusedPane.value).toBe('thread');
    } finally {
      (globalThis as any).innerWidth = origWidth;
    }
  });

  it('desktop: leaves drawer focus alone (drawer ↑/↓ browsing undisturbed)', () => {
    const origWidth = globalThis.innerWidth;
    (globalThis as any).innerWidth = 1024;
    try {
      focusedPane.value = 'drawer'; // browsing the thread list via keyboard
      focusThread('t1');
      // Only the cross-group (content) case switches; an intra-Threads-group
      // focus must survive so Enter-to-peek keeps the drawer accent + arrow nav.
      expect(focusedPane.value).toBe('drawer');
    } finally {
      (globalThis as any).innerWidth = origWidth;
    }
  });

  it('mobile: never touches focusedPane (panes are navigated, not focused)', () => {
    const origWidth = globalThis.innerWidth;
    (globalThis as any).innerWidth = 375; // mobile
    try {
      focusedPane.value = 'content';
      focusThread('t1');
      expect(focusedPane.value).toBe('content'); // unchanged on mobile
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

  it('desktop: reveals the thread pane group when arriving from content', () => {
    focusedPane.value = 'content'; // user was viewing an app/Settings
    unfocusThread();
    expect(focusedPane.value).toBe('thread');
  });

  it('mobile: swipes to the thread pane (new-chat / compose lands there)', () => {
    const origWidth = globalThis.innerWidth;
    (globalThis as any).innerWidth = 375;
    try {
      mobileView.value = 'content';
      unfocusThread();
      expect(mobileView.value).toBe('thread');
    } finally {
      (globalThis as any).innerWidth = origWidth;
    }
  });

  it('revealPane:false leaves the visible pane alone (stale-pointer cleanup)', () => {
    // ThreadView's render-phase cleanup passes this so a background-mounted pane
    // on mobile can't yank a user off the content pane.
    const origWidth = globalThis.innerWidth;
    (globalThis as any).innerWidth = 375;
    try {
      mobileView.value = 'content';
      unfocusThread({ revealPane: false });
      expect(mobileView.value).toBe('content');
    } finally {
      (globalThis as any).innerWidth = origWidth;
    }
  });
});

// ---------------------------------------------------------------------------
// Per-thread CC preferences — reset on thread switch
// ---------------------------------------------------------------------------

describe('CC pending preferences reset on thread switch', () => {
  it('focusThread resets pending CC model and reasoning effort', () => {
    codingAgentPendingModel.value = 'opus';
    codingAgentPendingReasoningEffort.value = 'max';

    focusThread('t1');

    expect(codingAgentPendingModel.value).toBeNull();
    expect(codingAgentPendingReasoningEffort.value).toBeNull();
  });

  it('switching between threads resets pending preferences', () => {
    focusThread('t1');
    codingAgentPendingModel.value = 'sonnet';
    codingAgentPendingReasoningEffort.value = 'low';

    focusThread('t2');

    expect(codingAgentPendingModel.value).toBeNull();
    expect(codingAgentPendingReasoningEffort.value).toBeNull();
  });

  it('unfocusThread resets pending CC preferences (compose view starts fresh)', () => {
    codingAgentPendingModel.value = 'opus';
    codingAgentPendingReasoningEffort.value = 'max';

    unfocusThread();

    expect(codingAgentPendingModel.value).toBeNull();
    expect(codingAgentPendingReasoningEffort.value).toBeNull();
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

  function mockApiResponse(threads: { thread_id: string; title: string; channel: string; last_activity: string; status?: string; coding_agent_proposed?: boolean; coding_agent_requires_restart?: boolean; coding_agent_is_external_repo?: boolean; coding_agent_applying?: boolean }[]) {
    (fetchThreads as any).mockResolvedValue({
      saved: [],
      active_threads: [],
      composing: [],
      family_threads: [],
      active: [],
      archive: threads.map(t => ({
        ...t,
        created_at: t.last_activity,
        message_count: 1,
        section: 'archived',
        status: t.status || 'idle',
        coding_agent_proposed: t.coding_agent_proposed || false,
        coding_agent_requires_restart: t.coding_agent_requires_restart || false,
        coding_agent_is_external_repo: t.coding_agent_is_external_repo || false,
        coding_agent_applying: t.coding_agent_applying || false,
        active_children_count: 0,
      })),
    });
  }

  it('stores the backend archive_count for the collapsed Archive badge', async () => {
    archiveThreadCount.value = 0;
    (fetchThreads as any).mockResolvedValue({
      saved: [], active_threads: [], composing: [], family_threads: [], active: [],
      archive: [],
      archive_count: 247,
    });

    await loadAllThreads();

    expect(archiveThreadCount.value).toBe(247);
  });

  it('resolves only after its eager per-thread loads settle, and bounds them', async () => {
    // The eager loads run through a pool now (they used to be one unbounded
    // `Promise.all`). Two things must survive that: callers ordering work after
    // `loadAllThreads` still see every load finished, and a boot no longer fires
    // one request per active thread at once.
    const ids = Array.from({ length: 12 }, (_, i) => `a${i}`);
    (fetchThreads as any).mockResolvedValue({
      saved: [], composing: [], family_threads: [], archive: [],
      active: ids,
      active_threads: ids.map(id => ({
        thread_id: id, title: id, channel: 'chat', initiator: 'user',
        last_activity: '2026-08-04T10:00:00Z', created_at: '2026-08-04T10:00:00Z',
        message_count: 1, section: 'inbox', status: 'idle',
        active_children_count: 0, total_children_count: 0,
        blocking_descendant_count: 0, attention_descendant_count: 0,
        coding_agent_proposed: false, coding_agent_requires_restart: false,
        coding_agent_is_external_repo: false, coding_agent_applying: false,
        coding_agent_has_diff: false, last_revived_at: null, state: 'active',
      })),
    });

    const { fetchThreadEvents } = await import('../../api/threads');
    const mock = fetchThreadEvents as unknown as ReturnType<typeof vi.fn>;
    mock.mockReset();
    let inFlight = 0;
    let peak = 0;
    let settled = 0;
    mock.mockImplementation(async () => {
      inFlight++;
      peak = Math.max(peak, inFlight);
      await Promise.resolve();
      inFlight--;
      settled++;
      return { events: [], currentAggregate: null };
    });

    await loadAllThreads();

    expect(settled).toBe(12);
    expect(peak).toBeLessThanOrEqual(4);
    mock.mockReset();
    mock.mockResolvedValue({ events: [], currentAggregate: null });
  });

  it('falls back to 0 when archive_count is omitted (older engine / mock)', async () => {
    archiveThreadCount.value = 99;
    (fetchThreads as any).mockResolvedValue({
      saved: [], active_threads: [], composing: [], family_threads: [], active: [],
      archive: [],
    });

    await loadAllThreads();

    expect(archiveThreadCount.value).toBe(0);
  });

  it('updates metadata for threads already in map (SSE skeletons)', async () => {
    // SSE skeleton has stale title but newer updatedAt from live events
    const map = new Map<string, ThreadState>();
    map.set('t1', makeThreadState('t1', {
      meta: { id: 't1', title: '...', channel: 'chat', saved: false, createdAt: '', updatedAt: '2026-03-15T17:00:00Z', status: 'idle', codingAgentProposed: false, codingAgentRequiresRestart: false, codingAgentIsExternalRepo: false, codingAgentApplying: false, lastRevivedAt: '', messageCount: 0, section: 'archived', activeChildrenCount: 0 },
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
      meta: { id: 't1', title: 'Generated Title', channel: 'chat', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:00Z', status: 'idle', codingAgentProposed: false, codingAgentRequiresRestart: false, codingAgentIsExternalRepo: false, codingAgentApplying: false, lastRevivedAt: '', messageCount: 0, section: 'archived', activeChildrenCount: 0 },
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
      meta: { id: 't1', title: 'Good Title From SSE', channel: 'chat', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:00Z', status: 'idle', codingAgentProposed: false, codingAgentRequiresRestart: false, codingAgentIsExternalRepo: false, codingAgentApplying: false, lastRevivedAt: '', messageCount: 0, section: 'archived', activeChildrenCount: 0 },
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
      meta: { id: 't1', title: '...', channel: 'chat', saved: false, createdAt: '', updatedAt: '2026-03-15T19:30:00Z', status: 'idle', codingAgentProposed: false, codingAgentRequiresRestart: false, codingAgentIsExternalRepo: false, codingAgentApplying: false, lastRevivedAt: '', messageCount: 0, section: 'archived', activeChildrenCount: 0 },
    }));
    map.set('t2', makeThreadState('t2', {
      meta: { id: 't2', title: '...', channel: 'chat', saved: false, createdAt: '', updatedAt: '2026-03-15T15:00:00Z', status: 'idle', codingAgentProposed: false, codingAgentRequiresRestart: false, codingAgentIsExternalRepo: false, codingAgentApplying: false, lastRevivedAt: '', messageCount: 0, section: 'archived', activeChildrenCount: 0 },
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
      archive: [
        { thread_id: 't1', title: 'Thread 1', channel: 'chat', last_activity: '2026-03-15T18:00:00Z', created_at: '2026-03-15T18:00:00Z', message_count: 5, section: 'archived', status: 'idle', coding_agent_proposed: false, coding_agent_requires_restart: false, coding_agent_is_external_repo: false, coding_agent_applying: false, last_revived_at: null, active_children_count: 0 },
        { thread_id: 't2', title: 'Thread 2', channel: 'claude_code', last_activity: '2026-03-15T17:00:00Z', created_at: '2026-03-15T17:00:00Z', message_count: 12, section: 'archived', status: 'idle', coding_agent_proposed: false, coding_agent_requires_restart: false, coding_agent_is_external_repo: false, coding_agent_applying: false, last_revived_at: null, active_children_count: 0 },
      ],
    });

    await loadAllThreads();

    expect(threadMap.value.get('t1')!.meta.messageCount).toBe(5);
    expect(threadMap.value.get('t2')!.meta.messageCount).toBe(12);
  });

  it('updates messageCount on existing threads from API', async () => {
    const map = new Map<string, ThreadState>();
    map.set('t1', makeThreadState('t1', {
      meta: { id: 't1', title: '...', channel: 'chat', saved: false, createdAt: '', updatedAt: '2026-03-15T19:00:00Z', status: 'idle', codingAgentProposed: false, codingAgentRequiresRestart: false, codingAgentIsExternalRepo: false, codingAgentApplying: false, lastRevivedAt: '', messageCount: 0, section: 'archived', activeChildrenCount: 0 },
    }));
    threadMap.value = map;

    (fetchThreads as any).mockResolvedValue({
      saved: [],
      active_threads: [],
      composing: [],
      active: [],
      archive: [
        { thread_id: 't1', title: 'Thread 1', channel: 'chat', last_activity: '2026-03-15T18:00:00Z', created_at: '2026-03-15T18:00:00Z', message_count: 7, section: 'archived', status: 'idle', coding_agent_proposed: false, coding_agent_requires_restart: false, coding_agent_is_external_repo: false, coding_agent_applying: false, last_revived_at: null, active_children_count: 0 },
      ],
    });

    await loadAllThreads();

    expect(threadMap.value.get('t1')!.meta.messageCount).toBe(7);
  });

  it('marks active threads from API active set', async () => {
    (fetchThreads as any).mockResolvedValue({
      saved: [],
      active_threads: [
        { thread_id: 't1', title: 'Active', channel: 'chat', last_activity: '2026-03-15T18:00:00Z', created_at: '2026-03-15T18:00:00Z', message_count: 1, section: 'archived', active_children_count: 0, status: 'running', coding_agent_proposed: false, coding_agent_requires_restart: false, coding_agent_is_external_repo: false, coding_agent_applying: false, last_revived_at: null },
      ],
      active: ['t1'],
      archive: [
        { thread_id: 't2', title: 'Idle', channel: 'chat', last_activity: '2026-03-15T17:00:00Z', created_at: '2026-03-15T17:00:00Z', message_count: 3, section: 'archived', active_children_count: 0, status: 'idle', coding_agent_proposed: false, coding_agent_requires_restart: false, coding_agent_is_external_repo: false, coding_agent_applying: false, last_revived_at: null },
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
        { thread_id: 't1', title: 'Trigger Run', channel: 'trigger', last_activity: '2026-03-15T18:00:00Z', created_at: '2026-03-15T18:00:00Z', message_count: 1, section: 'archived', status: 'running', coding_agent_proposed: false, coding_agent_requires_restart: false, coding_agent_is_external_repo: false, coding_agent_applying: false, last_revived_at: null, active_children_count: 0 },
      ],
      active: ['t1'],
      archive: [],
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
      archive: [{
        thread_id: threadId,
        title: 'T',
        channel: 'chat',
        last_activity: '2026-03-15T18:00:00Z',
        created_at: '2026-03-15T18:00:00Z',
        message_count: 0,
        section: 'archived',
        status: 'idle',
        coding_agent_proposed: false,
        coding_agent_requires_restart: false,
        coding_agent_is_external_repo: false,
        coding_agent_applying: false,
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

  it('preserves local composeImages when the user attached them here (locally edited)', async () => {
    const { composeEditedAt } = await import('./compose');
    const map = new Map<string, ThreadState>();
    map.set('t1', makeThreadState('t1', { meta: { state: 'composing', composeImages: ['local-img-base64'] } }));
    threadMap.value = map;
    focusedThreadId.value = 't1';
    focusPromptOnThread('t1');
    // A real image attach goes through updateCompose → markLocallyEdited, so the
    // draft carries a composeEditedAt stamp. Authorship (not focus) is what
    // protects it from an empty background snapshot.
    composeEditedAt.set('t1', Date.now());

    mockComposeApiResponse('t1', '', []); // empty server snapshot (stale / pre-attach)

    try {
      await loadAllThreads();
      expect(getDraft('t1').image_hashes).toEqual(['local-img-base64']);
    } finally {
      composeEditedAt.delete('t1');
      unfocusPrompt();
    }
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
        archive: [{
          thread_id: 't1',
          title: 'T',
          channel: 'chat',
          last_activity: '2026-03-15T18:00:00Z',
          created_at: '2026-03-15T18:00:00Z',
          message_count: 0,
          section: 'archived',
          status: 'idle',
          coding_agent_proposed: false,
          coding_agent_requires_restart: false,
          coding_agent_is_external_repo: false,
          coding_agent_applying: false,
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

  // "thread draft persists when switching to compose and back" (drafts.spec.ts:65)
  // flake. The previous two tests cover the case where the edit happens AT or
  // AFTER the GET went out (composeEditedAt >= requestStartedAt catches it). This
  // covers the INVERSE order: the user types the draft FIRST, then a resync
  // loadAllThreads starts, and its slow GET — whose server snapshot was read
  // before the debounced PUT committed — lands after the PUT settled and
  // pendingComposePuts cleared. Now composeEditedAt is OLDER than
  // requestStartedAt, so neither the focus, pending-PUT, nor edited-since-request
  // guard fires; only the PUT-settle guard prevents the stale empty snapshot
  // from blanking the restored draft.
  it('preserves local composeText when the edit precedes a resync whose stale response lands after the PUT settles', async () => {
    const { updateCompose, pendingComposePuts, composeEditedAt, composePutSettledAt } = await import('./compose');

    const map = new Map<string, ThreadState>();
    map.set('t1', makeThreadState('t1', { meta: { state: 'active', composeText: '' } }));
    threadMap.value = map;
    focusedThreadId.value = 't1';
    unfocusPrompt(); // user clicked away (compose) and back — textarea not focused

    // 1. User types the follow-up draft FIRST: optimistic local write, pending
    //    mark, 250ms debounced PUT.
    updateCompose('t1', { text: 'thread draft text' });
    expect(pendingComposePuts.has('t1')).toBe(true);

    // 2. A real delay so the resync's requestStartedAt is STRICTLY after the
    //    edit — otherwise composeEditedAt >= requestStartedAt would mask the hole
    //    via the existing guard, not the PUT-settle one under test.
    await new Promise((r) => setTimeout(r, 20));

    // 3. Resync loadAllThreads starts; its GET is gated to land late and carries
    //    the server's pre-PUT empty snapshot.
    let releaseFetch: (() => void) | null = null;
    const fetchGate = new Promise<void>((resolve) => { releaseFetch = resolve; });
    (fetchThreads as any).mockImplementationOnce(async () => {
      await fetchGate;
      return {
        saved: [],
        active_threads: [],
        composing: [],
        active: [],
        archive: [{
          thread_id: 't1',
          title: 'T',
          channel: 'chat',
          last_activity: '2026-03-15T18:00:00Z',
          created_at: '2026-03-15T18:00:00Z',
          message_count: 0,
          section: 'archived',
          status: 'idle',
          coding_agent_proposed: false,
          coding_agent_requires_restart: false,
          coding_agent_is_external_repo: false,
          coding_agent_applying: false,
          last_revived_at: null,
          active_children_count: 0,
          state: 'active',
          compose_text: '', // STALE — read before the PUT committed
          compose_images: [],
          compose_mode: null,
        }],
      };
    });

    try {
      // Kick off loadAllThreads; its HTTP is gated in flight.
      const loadPromise = loadAllThreads();

      // 4. Drain the debounce + pushNow so the PUT settles AFTER the GET started.
      //    pendingComposePuts clears and composePutSettledAt is stamped.
      await new Promise((r) => setTimeout(r, 300));
      expect(pendingComposePuts.has('t1')).toBe(false);
      expect(composePutSettledAt.get('t1') ?? 0).toBeGreaterThan(0);

      // 5. The stale empty response lands now. Without the PUT-settle guard,
      //    upsertThread blanks the draft (the drafts:65 "got ''" flake).
      releaseFetch!();
      await loadPromise;

      expect(getDraft('t1').text).toBe('thread draft text');
    } finally {
      composeEditedAt.delete('t1');
      composePutSettledAt.delete('t1');
      pendingComposePuts.delete('t1');
    }
  });

  // drafts.spec.ts:65 value='' — the RESIDUAL empty-clobber the composePutSettledAt
  // guard does NOT catch. The previous test covers a resync whose GET fired BEFORE
  // the PUT settled (composePutSettledAt >= requestStartedAt saves it). This covers
  // a compose PUT that never actually persisted (failed / timed out under host
  // contention — composePutSettledAt is stamped even then) followed by a resync
  // whose GET fired AFTER that settle stamp: composePutSettledAt < requestStartedAt
  // and composeEditedAt < requestStartedAt, so neither timing guard fires, the
  // textarea isn't focused, no PUT is pending — and the server (which never got the
  // text) returns an empty compose. Only the "never clear a non-empty locally-edited
  // draft via a bulk snapshot" rule in stageDraftFromApi preserves it.
  it('preserves a locally-edited draft when an empty resync lands AFTER the PUT settled (failed/never-committed PUT)', async () => {
    const { updateCompose, pendingComposePuts, composeEditedAt, composePutSettledAt } = await import('./compose');

    const map = new Map<string, ThreadState>();
    map.set('t1', makeThreadState('t1', { meta: { state: 'active', composeText: '' } }));
    threadMap.value = map;
    focusedThreadId.value = 't1';
    unfocusPrompt(); // user clicked away (compose) and back — textarea not focused

    // 1. User types the follow-up draft: optimistic local write, pending mark,
    //    250ms debounced PUT.
    updateCompose('t1', { text: 'thread draft text' });
    expect(pendingComposePuts.has('t1')).toBe(true);

    try {
      // 2. Drain the debounce so the PUT settles FIRST. composePutSettledAt is
      //    stamped and pendingComposePuts clears — exactly the post-settle state a
      //    failed/never-committed PUT also leaves behind (the server has no text).
      await new Promise((r) => setTimeout(r, 300));
      expect(pendingComposePuts.has('t1')).toBe(false);
      expect(composePutSettledAt.get('t1') ?? 0).toBeGreaterThan(0);

      // 3. A resync starts NOW — its requestStartedAt is strictly AFTER the settle
      //    stamp, so putSettledSinceRequest is false (the guard the prior test
      //    relies on does NOT fire here). The server snapshot is empty.
      mockComposeApiResponse('t1', '');
      await loadAllThreads();

      // Without the stageDraftFromApi rule, this empty snapshot blanks the draft
      // (the drafts:65 "got ''" flake). With it, the locally-typed draft survives.
      expect(getDraft('t1').text).toBe('thread draft text');
    } finally {
      composeEditedAt.delete('t1');
      composePutSettledAt.delete('t1');
      pendingComposePuts.delete('t1');
    }
  });

  // The guard is scoped to LOCALLY-edited drafts: a server-originated draft
  // (loaded cross-device, never edited on this device → no composeEditedAt entry)
  // must STILL be clearable by a bulk snapshot, so a peer's clear that arrives via
  // a resync (e.g. the SSE clear was missed while disconnected) still applies.
  it('still clears a server-originated draft (never edited here) when the snapshot is empty', async () => {
    const map = new Map<string, ThreadState>();
    map.set('t1', makeThreadState('t1', { meta: { state: 'active', composeText: 'from a peer device' } }));
    threadMap.value = map;
    unfocusPrompt();
    // No updateCompose on this device → composeEditedAt has no entry for t1.

    mockComposeApiResponse('t1', ''); // peer cleared it; resync carries empty

    await loadAllThreads();

    expect(getDraft('t1').text).toBe('');
  });

  // Sibling of the SSE fix: an EMPTY snapshot (the shared draft was sent/discarded
  // elsewhere) must clear a server-originated draft EVEN when the textarea is
  // focused here — focus alone must not preserve a draft the user never typed.
  // The non-empty focus guard (test at "preserves local composeText when textarea
  // is focused") is unaffected. A locally-edited draft is still protected by
  // stageDraftFromApi's hasUnsentLocalDraft guard (the failed-PUT test above).
  it('clears a focused server-originated draft when the resync snapshot is empty (peer sent elsewhere)', async () => {
    const map = new Map<string, ThreadState>();
    map.set('t1', makeThreadState('t1', { meta: { state: 'active', composeText: 'follow-up drafted on my other device' } }));
    threadMap.value = map;
    focusedThreadId.value = 't1';
    focusPromptOnThread('t1');
    // No updateCompose here → composeEditedAt has no entry for t1 (server-originated).

    mockComposeApiResponse('t1', ''); // peer's send cleared compose_text server-side

    await loadAllThreads();

    expect(getDraft('t1').text).toBe('');
    unfocusPrompt();
  });
});

// ---------------------------------------------------------------------------
// ensureThreadInMap — search result click bootstrapping
// ---------------------------------------------------------------------------

