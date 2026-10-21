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
import { fetchThreads, type CodingAgentChangeState } from '../../api/threads';
import * as scrollState from '../../components/chat/scrollState';
import { awayFromBottom, followSurvivesScroll, notAtTop, setActiveScrollElement, setFollowLiveEdge, stopFollowingBottom } from '../../components/chat/scrollState';
import { provideScrollState } from '../../components/chat/scrollStateHandle';
import { drawerOpen } from '../../components/layout/drawerState';
import { threadScrollKey } from '../savedScroll';
import { _resetComposeDraftsForTesting, getDraft } from '../composeDrafts';
import { archiveThreadCount, codingAgentPendingModel, codingAgentPendingReasoningEffort, focusedPane, focusedThreadId, generatedTitleIds, mobileView, resetCodingAgentPendingPreferences, threadDrawerOpen, threadMap, threadsLoaded, THREAD_EVENTS_FETCH_CONCURRENCY, THREAD_EVENTS_PREFETCH_LIMIT } from '../store';
import { _resetThreadOpenMarksForTesting, takeThreadOpenStart } from '../../utils/threadOpenMarks';
import { closeDashboardSheet, dashboardSheetOpen, openDashboardSheet } from '../dashboards';
import { loadAllThreads, loadThreadEvents } from './thread-loading';
import { focusThread, handleSaveThread, unfocusThread } from './threads';
import { _resetRowIntentsForTesting } from '../rowIntents';
import { _resetComposeSyncForTesting } from '../composeSync';

// Mock the API module
vi.mock('../../api/threads', () => ({
  fetchThreads: vi.fn(),
  fetchThreadEvents: vi.fn().mockResolvedValue({ events: [], currentAggregate: null }),
  fetchThreadMessages: vi.fn(),
  saveThread: vi.fn().mockResolvedValue({ summary_version: 1 }),
  archiveThread: vi.fn().mockResolvedValue({ archived: [] }),
}));

// Use the real isComposeFocusedHere — the compose-preservation tests below
// plant a fake activeElement + querySelectorAll to drive its branches. Mocking
// only the focus-side-effect helpers keeps the predicate honest if it grows.
vi.mock('../../components/chat/promptFocus', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../components/chat/promptFocus')>()),
  focusPromptNow: vi.fn(),
}));

vi.mock('../../api/client', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../api/client')>()),
  API_BASE: '',
  submitChat: vi.fn().mockResolvedValue({ event_id: 'e-1' }),
  cancelChat: vi.fn(),
  stopClaudeCode: vi.fn(),
  putComposeOnThread: vi.fn().mockResolvedValue({ status: 'applied' }),
  ensureThreadStarted: vi.fn().mockResolvedValue(undefined),
  deleteThread: vi.fn().mockResolvedValue(undefined),
}));

// ---------------------------------------------------------------------------
// Setup
// ---------------------------------------------------------------------------

// The store reaches scrollState through the handle, which the shell fills.
provideScrollState(scrollState);

beforeEach(() => {
  threadMap.value = new Map();
  _resetComposeDraftsForTesting();
  focusedThreadId.value = null;
  closeDashboardSheet();
  mobileView.value = 'thread';
  threadDrawerOpen.value = false;
  drawerOpen.value = false;
  resetCodingAgentPendingPreferences();
  generatedTitleIds.clear();
  _resetRowIntentsForTesting();
  focusedPane.value = 'thread';
  localStorage.removeItem('lucidos-focused-thread');
});

describe('focusThread', () => {
  it('sets focusedThreadId', () => {
    focusThread('t1');
    expect(focusedThreadId.value).toBe('t1');
  });

  it('closes the dashboard sheet, moving to another thread or not', () => {
    focusThread('t1');
    openDashboardSheet();
    focusThread('t2');
    expect(dashboardSheetOpen.value).toBe(false);
    openDashboardSheet();
    focusThread('t2');
    expect(dashboardSheetOpen.value).toBe(false);
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
        expect(followSurvivesScroll(el)).toBe(true);

        focusThread('t2');
        expect(followSurvivesScroll(el)).toBe(false);
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
        expect(followSurvivesScroll(el)).toBe(true);
      });
    });

    it('retires it when the reader leaves for the compose view', () => {
      // The compose view has its own scroll container and registers itself as
      // the active one, so a follow left armed would ride its growth instead.
      withTranscript((el) => {
        focusThread('t1');
        setFollowLiveEdge(true);

        unfocusThread();
        expect(followSurvivesScroll(el)).toBe(false);
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

// ---------------------------------------------------------------------------
// The perf open mark
//
// `thread-render` is the only sample that measures a thread open, and the mark
// is what arms it. The mark used to be stamped under `loadThreadEvents`'
// `eventsLoaded` early return, so only a thread's FIRST open of the session
// recorded anything. Back / forward between visited threads was invisible,
// which is the navigation a reader is most likely to call slow.
// ---------------------------------------------------------------------------

describe('the perf open mark', () => {
  beforeEach(() => _resetThreadOpenMarksForTesting());

  function withThread(id: string, eventsLoaded: boolean) {
    const map = new Map<string, ThreadState>();
    map.set(id, makeThreadState(id, { eventsLoaded }));
    threadMap.value = map;
  }

  it('stamps a WARM open, so thread-to-thread navigation is measured', () => {
    withThread('warm', true);

    focusThread('warm');

    expect(takeThreadOpenStart('warm')?.warm).toBe(true);
  });

  it('stamps a COLD open, and says it is the fetching kind', () => {
    withThread('cold', false);

    focusThread('cold');

    expect(takeThreadOpenStart('cold')?.warm).toBe(false);
  });

  it('leaves no mark on a thread nobody is looking at', () => {
    // loadAllThreads eagerly loads non-focused threads. A mark left on one
    // would be taken by a much later render and report a renderMs in minutes.
    withThread('background', true);
    focusedThreadId.value = 'somewhere-else';

    void loadThreadEvents('background');

    expect(takeThreadOpenStart('background')).toBeUndefined();
  });

  // The two below are the same failure reached two ways: a mark laid where no
  // render follows. The next streamed event changes the exchange count and
  // consumes it, so the sample reports however long the reader sat still as a
  // render. Only a focus TRANSITION may stamp.

  it('does not re-stamp when the reader re-taps the thread already open', () => {
    withThread('t', true);
    focusThread('t');
    expect(takeThreadOpenStart('t')?.warm).toBe(true); // the real open

    focusThread('t'); // re-tap: renders nothing

    expect(takeThreadOpenStart('t')).toBeUndefined();
  });

  it('does not stamp when a resync reloads the focused thread', () => {
    // loadAllThreads reloads the focused thread on every SSE reopen, Lagged
    // and iOS PWA wake. A warm one fetches nothing, so it is not an open.
    withThread('t', true);
    focusThread('t');
    takeThreadOpenStart('t'); // consume the real open's mark

    void loadThreadEvents('t');

    expect(takeThreadOpenStart('t')).toBeUndefined();
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

  function mockApiResponse(threads: { thread_id: string; title: string; channel: string; last_activity: string; status?: string; coding_agent_change_state?: CodingAgentChangeState; coding_agent_is_external_repo?: boolean }[]) {
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
        coding_agent_change_state: t.coding_agent_change_state ?? { kind: 'none' },
        coding_agent_is_external_repo: t.coding_agent_is_external_repo || false,
        active_children_count: 0,
      })),
    });
  }

  it('stores the backend archive_count for the Archive badge', async () => {
    archiveThreadCount.value = 0;
    (fetchThreads as any).mockResolvedValue({
      saved: [], active_threads: [], composing: [], family_threads: [], active: [],
      archive: [],
      archive_count: 247,
    });

    await loadAllThreads();

    expect(archiveThreadCount.value).toBe(247);
  });

  /** Mock a boot carrying `count` active threads, newest first by id order. */
  function activeThreadsResponse(count: number) {
    const ids = Array.from({ length: count }, (_, i) => `a${i}`);
    (fetchThreads as any).mockResolvedValue({
      saved: [], composing: [], family_threads: [], archive: [],
      active: ids,
      active_threads: ids.map((id, i) => ({
        thread_id: id, title: id, channel: 'chat', initiator: 'user',
        // Descending, so the prefetch's newest-first sort has something to sort.
        last_activity: `2026-08-04T10:${String(59 - i).padStart(2, '0')}:00Z`,
        created_at: '2026-08-04T10:00:00Z',
        message_count: 1, section: 'inbox', status: 'idle',
        active_children_count: 0, total_children_count: 0,
        blocking_descendant_count: 0, attention_descendant_count: 0,
        coding_agent_change_state: { kind: 'none' },
        coding_agent_is_external_repo: false,
        last_revived_at: null, state: 'active',
      })),
    });
    return ids;
  }

  /** Instrument `fetchThreadEvents` with the concurrency and order it saw. */
  async function recordEventFetches() {
    const { fetchThreadEvents } = await import('../../api/threads');
    const mock = fetchThreadEvents as unknown as ReturnType<typeof vi.fn>;
    mock.mockReset();
    const seen: string[] = [];
    let inFlight = 0;
    const stats = { peak: 0, settled: 0, seen };
    mock.mockImplementation(async (threadId: string) => {
      seen.push(threadId);
      inFlight++;
      stats.peak = Math.max(stats.peak, inFlight);
      await Promise.resolve();
      inFlight--;
      stats.settled++;
      return { events: [], currentAggregate: null };
    });
    return {
      stats,
      restore: () => {
        mock.mockReset();
        mock.mockResolvedValue({ events: [], currentAggregate: null });
      },
    };
  }

  it('resolves only after its eager per-thread loads settle, and bounds them', async () => {
    // The eager loads run through a pool (they used to be one unbounded
    // `Promise.all`), and the pool is now CAPPED. Three things must hold. A
    // caller ordering work after `loadAllThreads` still sees every load it
    // started finished. A boot fires no request-per-active-thread burst. And
    // the burst is bounded in TOTAL, not only in width.
    activeThreadsResponse(12);
    const { stats, restore } = await recordEventFetches();

    await loadAllThreads();

    // No focused thread here, so every fetch came from the capped prefetch.
    expect(stats.settled).toBe(THREAD_EVENTS_PREFETCH_LIMIT);
    expect(stats.peak).toBeLessThanOrEqual(THREAD_EVENTS_FETCH_CONCURRENCY);
    restore();
  });

  it('prefetches the most recently active threads, not an arbitrary slice', async () => {
    // The cap only helps if it keeps the threads worth keeping. `a0` is newest.
    activeThreadsResponse(12);
    const { stats, restore } = await recordEventFetches();

    await loadAllThreads();

    expect([...stats.seen].sort()).toEqual(
      Array.from({ length: THREAD_EVENTS_PREFETCH_LIMIT }, (_, i) => `a${i}`).sort(),
    );
    restore();
  });

  it('loads the focused thread first and alone, ahead of the prefetch', async () => {
    // The reader is looking at exactly one thread, and it used to share a
    // four-wide pool with every active and saved one. So its transcript
    // competed for bandwidth with a pile nobody asked for, which is the whole
    // reported symptom on a phone.
    const ids = activeThreadsResponse(12);
    focusedThreadId.value = ids[5];
    const { stats, restore } = await recordEventFetches();

    await loadAllThreads();

    // First, and on its own: the prefetch rounds that follow are what raise
    // the peak above one.
    expect(stats.seen[0]).toBe(ids[5]);
    expect(stats.settled).toBe(THREAD_EVENTS_PREFETCH_LIMIT + 1);
    restore();
    focusedThreadId.value = null;
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
      meta: { id: 't1', title: '...', channel: 'chat', saved: false, createdAt: '', updatedAt: '2026-03-15T17:00:00Z', status: 'idle', codingAgentChangeState: { kind: 'none' }, codingAgentIsExternalRepo: false, lastRevivedAt: '', messageCount: 0, section: 'archived', activeChildrenCount: 0 },
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
      meta: { id: 't1', title: 'Generated Title', channel: 'chat', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:00Z', status: 'idle', codingAgentChangeState: { kind: 'none' }, codingAgentIsExternalRepo: false, lastRevivedAt: '', messageCount: 0, section: 'archived', activeChildrenCount: 0 },
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
      meta: { id: 't1', title: 'Good Title From SSE', channel: 'chat', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:00Z', status: 'idle', codingAgentChangeState: { kind: 'none' }, codingAgentIsExternalRepo: false, lastRevivedAt: '', messageCount: 0, section: 'archived', activeChildrenCount: 0 },
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
      meta: { id: 't1', title: '...', channel: 'chat', saved: false, createdAt: '', updatedAt: '2026-03-15T19:30:00Z', status: 'idle', codingAgentChangeState: { kind: 'none' }, codingAgentIsExternalRepo: false, lastRevivedAt: '', messageCount: 0, section: 'archived', activeChildrenCount: 0 },
    }));
    map.set('t2', makeThreadState('t2', {
      meta: { id: 't2', title: '...', channel: 'chat', saved: false, createdAt: '', updatedAt: '2026-03-15T15:00:00Z', status: 'idle', codingAgentChangeState: { kind: 'none' }, codingAgentIsExternalRepo: false, lastRevivedAt: '', messageCount: 0, section: 'archived', activeChildrenCount: 0 },
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
        { thread_id: 't1', title: 'Thread 1', channel: 'chat', last_activity: '2026-03-15T18:00:00Z', created_at: '2026-03-15T18:00:00Z', message_count: 5, section: 'archived', status: 'idle', coding_agent_change_state: { kind: 'none' }, coding_agent_is_external_repo: false, last_revived_at: null, active_children_count: 0 },
        { thread_id: 't2', title: 'Thread 2', channel: 'claude_code', last_activity: '2026-03-15T17:00:00Z', created_at: '2026-03-15T17:00:00Z', message_count: 12, section: 'archived', status: 'idle', coding_agent_change_state: { kind: 'none' }, coding_agent_is_external_repo: false, last_revived_at: null, active_children_count: 0 },
      ],
    });

    await loadAllThreads();

    expect(threadMap.value.get('t1')!.meta.messageCount).toBe(5);
    expect(threadMap.value.get('t2')!.meta.messageCount).toBe(12);
  });

  it('updates messageCount on existing threads from API', async () => {
    const map = new Map<string, ThreadState>();
    map.set('t1', makeThreadState('t1', {
      meta: { id: 't1', title: '...', channel: 'chat', saved: false, createdAt: '', updatedAt: '2026-03-15T19:00:00Z', status: 'idle', codingAgentChangeState: { kind: 'none' }, codingAgentIsExternalRepo: false, lastRevivedAt: '', messageCount: 0, section: 'archived', activeChildrenCount: 0 },
    }));
    threadMap.value = map;

    (fetchThreads as any).mockResolvedValue({
      saved: [],
      active_threads: [],
      composing: [],
      active: [],
      archive: [
        { thread_id: 't1', title: 'Thread 1', channel: 'chat', last_activity: '2026-03-15T18:00:00Z', created_at: '2026-03-15T18:00:00Z', message_count: 7, section: 'archived', status: 'idle', coding_agent_change_state: { kind: 'none' }, coding_agent_is_external_repo: false, last_revived_at: null, active_children_count: 0 },
      ],
    });

    await loadAllThreads();

    expect(threadMap.value.get('t1')!.meta.messageCount).toBe(7);
  });

  it('marks active threads from API active set', async () => {
    (fetchThreads as any).mockResolvedValue({
      saved: [],
      active_threads: [
        { thread_id: 't1', title: 'Active', channel: 'chat', last_activity: '2026-03-15T18:00:00Z', created_at: '2026-03-15T18:00:00Z', message_count: 1, section: 'archived', active_children_count: 0, status: 'running', coding_agent_change_state: { kind: 'none' }, coding_agent_is_external_repo: false, last_revived_at: null },
      ],
      active: ['t1'],
      archive: [
        { thread_id: 't2', title: 'Idle', channel: 'chat', last_activity: '2026-03-15T17:00:00Z', created_at: '2026-03-15T17:00:00Z', message_count: 3, section: 'archived', active_children_count: 0, status: 'idle', coding_agent_change_state: { kind: 'none' }, coding_agent_is_external_repo: false, last_revived_at: null },
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
        { thread_id: 't1', title: 'Trigger Run', channel: 'trigger', last_activity: '2026-03-15T18:00:00Z', created_at: '2026-03-15T18:00:00Z', message_count: 1, section: 'archived', status: 'running', coding_agent_change_state: { kind: 'none' }, coding_agent_is_external_repo: false, last_revived_at: null, active_children_count: 0 },
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
    _resetComposeSyncForTesting();
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
        coding_agent_change_state: { kind: 'none' },
        coding_agent_is_external_repo: false,
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

  /** A local edit is a *pending intent* until a write covering it is
   *  answered (ADR 0437). No snapshot replaces it meanwhile, focused or not.
   *  The orderings by revision are pinned in
   *  `store/__tests__/compose-follows-server-revision.test.ts`. */
  it('a pending local edit holds over a snapshot, text and images alike', async () => {
    const { updateCompose } = await import('./compose');
    const map = new Map<string, ThreadState>();
    map.set('t1', makeThreadState('t1', { meta: { state: 'composing' } }));
    threadMap.value = map;
    unfocusPrompt();
    updateCompose('t1', { text: 'hello world I am typing', image_hashes: ['local-img-hash'] });

    mockComposeApiResponse('t1', 'hello', []);
    await loadAllThreads();

    expect(getDraft('t1').text).toBe('hello world I am typing');
    expect(getDraft('t1').image_hashes).toEqual(['local-img-hash']);
  });

  /** Nothing pending: a newer snapshot is the engine's draft, and dropping it
   *  because the box has focus would let the next keystroke overwrite it. */
  it('a newer snapshot applies to a focused composer with nothing pending', async () => {
    const map = new Map<string, ThreadState>();
    map.set('t1', makeThreadState('t1', { meta: { state: 'composing', composeText: 'hello' } }));
    threadMap.value = map;
    focusedThreadId.value = 't1';
    focusPromptOnThread('t1');

    mockComposeApiResponse('t1', 'hello from my other device');
    await loadAllThreads();

    expect(getDraft('t1').text).toBe('hello from my other device');
    unfocusPrompt();
  });

  // A peer's clear that arrives via a resync (the SSE clear was missed while
  // disconnected) applies when nothing is pending here.
  it('still clears a server-originated draft (never edited here) when the snapshot is empty', async () => {
    const map = new Map<string, ThreadState>();
    map.set('t1', makeThreadState('t1', { meta: { state: 'active', composeText: 'from a peer device' } }));
    threadMap.value = map;
    unfocusPrompt();

    mockComposeApiResponse('t1', ''); // peer cleared it; resync carries empty

    await loadAllThreads();

    expect(getDraft('t1').text).toBe('');
  });

  // An EMPTY snapshot (the shared draft was sent or discarded elsewhere) clears
  // the draft even when the textarea is focused here.
  it('clears a focused server-originated draft when the resync snapshot is empty (peer sent elsewhere)', async () => {
    const map = new Map<string, ThreadState>();
    map.set('t1', makeThreadState('t1', { meta: { state: 'active', composeText: 'follow-up drafted on my other device' } }));
    threadMap.value = map;
    focusedThreadId.value = 't1';
    focusPromptOnThread('t1');

    mockComposeApiResponse('t1', ''); // peer's send cleared compose_text server-side

    await loadAllThreads();

    expect(getDraft('t1').text).toBe('');
    unfocusPrompt();
  });
});

// ---------------------------------------------------------------------------
// ensureThreadInMap — search result click bootstrapping
// ---------------------------------------------------------------------------

