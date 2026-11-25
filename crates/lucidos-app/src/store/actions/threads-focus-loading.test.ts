import { describe, it, expect, beforeEach, vi } from 'vitest';
import { effect } from '@preact/signals';

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
import { fetchThreads, type CodingAgentChangeState } from '../../api/threads';
import * as scrollState from '../../components/chat/scrollState';
import { awayFromBottom, classifyScroll, notAtTop, setActiveScrollElement, setFollowLiveEdge, stopFollowingBottom } from '../../components/chat/scrollState';
import { provideScrollState } from '../../components/chat/scrollStateHandle';
import { drawerOpen } from '../../components/layout/drawerState';
import { threadScrollKey } from '../savedScroll';
import { _resetComposeDraftsForTesting, getDraft } from '../composeDrafts';
import { archiveThreadCount, focusedPane, focusedThreadId, generatedTitleIds, mobileView, threadDrawerOpen, threadMap, threadsLoaded, THREAD_EVENTS_FETCH_CONCURRENCY, THREAD_EVENTS_PREFETCH_LIMIT } from '../store';
import { _resetThreadOpenMarksForTesting, takeThreadOpenStart } from '../../utils/threadOpenMarks';
import { closeDashboardPanel, dashboardPanelOpen, openDashboardPanel } from '../dashboards';
import { loadAllThreads, loadThreadEvents } from './thread-loading';
import { focusThread, focusThreadOrBootstrapResult, handleSaveThread, unfocusThread } from './threads';
import { _resetRowIntentsForTesting } from '../rowIntents';
import { _resetComposeSyncForTesting } from '../composeSync';

vi.mock('../../api/threads', () => ({
  fetchThreads: vi.fn(),
  fetchThreadEvents: vi.fn().mockResolvedValue({ events: [], currentAggregate: null }),
  fetchThreadMessages: vi.fn(),
  saveThread: vi.fn().mockResolvedValue({ summary_version: 1 }),
  archiveThread: vi.fn().mockResolvedValue({ archived: [] }),
}));

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


provideScrollState(scrollState);

beforeEach(() => {
  threadMap.value = new Map();
  _resetComposeDraftsForTesting();
  focusedThreadId.value = null;
  closeDashboardPanel();
  mobileView.value = 'thread';
  threadDrawerOpen.value = false;
  drawerOpen.value = false;
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

  it('closes the dashboard panel, moving to another thread or not', () => {
    focusThread('t1');
    openDashboardPanel();
    focusThread('t2');
    expect(dashboardPanelOpen.value).toBe(false);
    openDashboardPanel();
    focusThread('t2');
    expect(dashboardPanelOpen.value).toBe(false);
  });

  it('does not move the transcript, whatever it finds there', () => {
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

  describe('the standing follow and the thread being opened', () => {
    function makeTranscript() {
      return {
        parentElement: null,
        scrollTop: 8500,
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
        setFollowLiveEdge(true);
        expect(classifyScroll(el).followSurvives).toBe(true);

        focusThread('t2');
        expect(classifyScroll(el).followSurvives).toBe(false);
      });
    });

    it('keeps it when the reader re-taps the thread they are already in', () => {
      withTranscript((el) => {
        focusThread('t1');
        setFollowLiveEdge(true);

        focusThread('t1');
        expect(classifyScroll(el).followSurvives).toBe(true);
      });
    });

    it('retires it when the reader leaves for the compose view', () => {
      withTranscript((el) => {
        focusThread('t1');
        setFollowLiveEdge(true);

        unfocusThread();
        expect(classifyScroll(el).followSurvives).toBe(false);
      });
    });

    it('retires it before the focus moves, so no subscriber sees the old follow still armed', () => {
      withTranscript((el) => {
        focusThread('t1');
        setFollowLiveEdge(true);
        const seen: boolean[] = [];
        const stop = effect(() => {
          if (focusedThreadId.value === 't2' && seen.length === 0) seen.push(classifyScroll(el).followSurvives);
        });

        focusThread('t2');
        stop();
        expect(seen).toEqual([false]);
      });
    });

    it('retires it at the optimistic focus when a bootstrap opens a thread outside the loaded list', () => {
      withTranscript((el) => {
        focusThread('t1');
        setFollowLiveEdge(true);

        void focusThreadOrBootstrapResult('t2').catch(() => {});
        expect(classifyScroll(el).followSurvives).toBe(false);
      });
    });
  });

  it('leaves the chevron signals to the scroll listener', () => {
    notAtTop.value = true;
    awayFromBottom.value = true;
    focusThread('t1');
    focusThread('t1');
    expect(notAtTop.value).toBe(true);
    expect(awayFromBottom.value).toBe(true);
  });

  it('navigates to thread pane on mobile', () => {
    const origWidth = globalThis.innerWidth;
    (globalThis as any).innerWidth = 375;
    try {
      mobileView.value = 'threads';
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
    (globalThis as any).innerWidth = 1024;
    try {
      mobileView.value = 'threads';
      focusThread('t1');
      expect(mobileView.value).toBe('threads');
    } finally {
      (globalThis as any).innerWidth = origWidth;
    }
  });

  it('desktop: re-activates the Threads pane group when arriving from content', () => {
    const origWidth = globalThis.innerWidth;
    (globalThis as any).innerWidth = 1024;
    try {
      focusedPane.value = 'content';
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
      focusedPane.value = 'drawer';
      focusThread('t1');
      expect(focusedPane.value).toBe('drawer');
    } finally {
      (globalThis as any).innerWidth = origWidth;
    }
  });

  it('mobile: never touches focusedPane (panes are navigated, not focused)', () => {
    const origWidth = globalThis.innerWidth;
    (globalThis as any).innerWidth = 375;
    try {
      focusedPane.value = 'content';
      focusThread('t1');
      expect(focusedPane.value).toBe('content');
    } finally {
      (globalThis as any).innerWidth = origWidth;
    }
  });

});


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
    withThread('background', true);
    focusedThreadId.value = 'somewhere-else';

    void loadThreadEvents('background');

    expect(takeThreadOpenStart('background')).toBeUndefined();
  });


  it('does not re-stamp when the reader re-taps the thread already open', () => {
    withThread('t', true);
    focusThread('t');
    expect(takeThreadOpenStart('t')?.warm).toBe(true);

    focusThread('t');

    expect(takeThreadOpenStart('t')).toBeUndefined();
  });

  it('does not stamp when a resync reloads the focused thread', () => {
    withThread('t', true);
    focusThread('t');
    takeThreadOpenStart('t');

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
    focusedPane.value = 'content';
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
    const { sendMessage } = await import('./chat');
    const { connectionStatus } = await import('../store');

    connectionStatus.value = 'connected';

    focusedThreadId.value = null;
    localStorage.removeItem('lucidos-focused-thread');

    await sendMessage('hello');

    expect(focusedThreadId.value).not.toBeNull();
    expect(localStorage.getItem('lucidos-focused-thread')).toBe(focusedThreadId.value);
  });
});


describe('handleSaveThread', () => {
  it('sets saved to true in threadMap', async () => {
    const map = new Map<string, ThreadState>();
    map.set('t1', makeThreadState('t1'));
    threadMap.value = map;

    await handleSaveThread('t1');

    expect(threadMap.value.get('t1')!.meta.saved).toBe(true);
  });
});


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

  function activeThreadsResponse(count: number) {
    const ids = Array.from({ length: count }, (_, i) => `a${i}`);
    (fetchThreads as any).mockResolvedValue({
      saved: [], composing: [], family_threads: [], archive: [],
      active: ids,
      active_threads: ids.map((id, i) => ({
        thread_id: id, title: id, channel: 'chat', initiator: 'user',
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
    activeThreadsResponse(12);
    const { stats, restore } = await recordEventFetches();

    await loadAllThreads();

    expect(stats.settled).toBe(THREAD_EVENTS_PREFETCH_LIMIT);
    expect(stats.peak).toBeLessThanOrEqual(THREAD_EVENTS_FETCH_CONCURRENCY);
    restore();
  });

  it('prefetches the most recently active threads, not an arbitrary slice', async () => {
    activeThreadsResponse(12);
    const { stats, restore } = await recordEventFetches();

    await loadAllThreads();

    expect([...stats.seen].sort()).toEqual(
      Array.from({ length: THREAD_EVENTS_PREFETCH_LIMIT }, (_, i) => `a${i}`).sort(),
    );
    restore();
  });

  it('loads the focused thread first and alone, ahead of the prefetch', async () => {
    const ids = activeThreadsResponse(12);
    focusedThreadId.value = ids[5];
    const { stats, restore } = await recordEventFetches();

    await loadAllThreads();

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
    expect(t1.meta.updatedAt).toBe('2026-03-15T18:30:00Z');
  });

  it('does not overwrite SSE-generated title with stale API data', async () => {
    const map = new Map<string, ThreadState>();
    map.set('t1', makeThreadState('t1', {
      meta: { id: 't1', title: 'Generated Title', channel: 'chat', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:00Z', status: 'idle', codingAgentChangeState: { kind: 'none' }, codingAgentIsExternalRepo: false, lastRevivedAt: '', messageCount: 0, section: 'archived', activeChildrenCount: 0 },
    }));
    threadMap.value = map;
    generatedTitleIds.add('t1');

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
    const map = new Map<string, ThreadState>();
    map.set('t1', makeThreadState('t1', {
      meta: { id: 't1', title: '...', channel: 'chat', saved: false, createdAt: '', updatedAt: '2026-03-15T19:30:00Z', status: 'idle', codingAgentChangeState: { kind: 'none' }, codingAgentIsExternalRepo: false, lastRevivedAt: '', messageCount: 0, section: 'archived', activeChildrenCount: 0 },
    }));
    map.set('t2', makeThreadState('t2', {
      meta: { id: 't2', title: '...', channel: 'chat', saved: false, createdAt: '', updatedAt: '2026-03-15T15:00:00Z', status: 'idle', codingAgentChangeState: { kind: 'none' }, codingAgentIsExternalRepo: false, lastRevivedAt: '', messageCount: 0, section: 'archived', activeChildrenCount: 0 },
    }));
    threadMap.value = map;

    mockApiResponse([
      { thread_id: 't1', title: 'Old Thread', channel: 'chat', last_activity: '2026-03-15T10:00:00Z' },
      { thread_id: 't2', title: 'Recent Thread', channel: 'claude_code', last_activity: '2026-03-15T18:00:00Z' },
    ]);

    await loadAllThreads();

    expect(threadMap.value.get('t1')!.meta.updatedAt).toBe('2026-03-15T19:30:00Z');
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
    const map = new Map<string, ThreadState>();
    map.set('t1', makeThreadState('t1'));
    threadMap.value = map;
    expect(threadsLoaded.value).toBe(false);
    expect(threadMap.value.size).toBe(1);
  });

  it('preserves API channel on initial load — thread_summaries source is authoritative', async () => {
    (fetchThreads as any).mockResolvedValue({
      saved: [],
      active_threads: [
        { thread_id: 't1', title: 'Trigger Run', channel: 'trigger', last_activity: '2026-03-15T18:00:00Z', created_at: '2026-03-15T18:00:00Z', message_count: 1, section: 'archived', status: 'running', coding_agent_change_state: { kind: 'none' }, coding_agent_is_external_repo: false, last_revived_at: null, active_children_count: 0 },
      ],
      active: ['t1'],
      archive: [],
      composing: [],
    });

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

    const thread = threadMap.value.get('t1')!;
    expect(thread.meta.channel).toBe('trigger');
  });
});


describe('loadAllThreads — compose preservation', () => {
  beforeEach(() => {
    threadsLoaded.value = false;
    _resetComposeSyncForTesting();
  });

  function mockComposeApiResponse(threadId: string, composeText: string, composeImages: string[] = []) {
    const response = {
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
    };
    (fetchThreads as any).mockResolvedValue(response);
    return response;
  }

  it.each([false, true])('saved-draft acceptance during a list read reconciles with pending edit=%s', async (localEdit) => {
    const { handleGlobalEvent } = await import('./thread-sync');
    const { setDraft } = await import('../composeDrafts');
    const { noteLocalComposeEdit } = await import('../composeSync');
    const stale = mockComposeApiResponse('t1', 'saved content', ['saved-image']);
    let release!: (response: typeof stale) => void;
    (fetchThreads as any).mockImplementationOnce(() => new Promise(resolve => { release = resolve; }));
    const reading = loadAllThreads();
    if (localEdit) {
      setDraft('t1', { text: 'later local edit', image_hashes: ['local-image'], mode: 'lucidos' });
      noteLocalComposeEdit('t1');
    }
    mockComposeApiResponse('t1', '');
    const readsBefore = vi.mocked(fetchThreads).mock.calls.length;
    handleGlobalEvent('DraftSubmissionAccepted', { thread_id: 't1' });
    release(stale);
    await reading;
    expect(vi.mocked(fetchThreads).mock.calls.length).toBe(readsBefore + 1);
    expect(getDraft('t1').text).toBe(localEdit ? 'later local edit' : '');
    expect(getDraft('t1').image_hashes).toEqual(localEdit ? ['local-image'] : []);
  });

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

  it('still clears a server-originated draft (never edited here) when the snapshot is empty', async () => {
    const map = new Map<string, ThreadState>();
    map.set('t1', makeThreadState('t1', { meta: { state: 'active', composeText: 'from a peer device' } }));
    threadMap.value = map;
    unfocusPrompt();

    mockComposeApiResponse('t1', '');

    await loadAllThreads();

    expect(getDraft('t1').text).toBe('');
  });

  it('clears a focused server-originated draft when the resync snapshot is empty (peer sent elsewhere)', async () => {
    const map = new Map<string, ThreadState>();
    map.set('t1', makeThreadState('t1', { meta: { state: 'active', composeText: 'follow-up drafted on my other device' } }));
    threadMap.value = map;
    focusedThreadId.value = 't1';
    focusPromptOnThread('t1');

    mockComposeApiResponse('t1', '');

    await loadAllThreads();

    expect(getDraft('t1').text).toBe('');
    unfocusPrompt();
  });
});
