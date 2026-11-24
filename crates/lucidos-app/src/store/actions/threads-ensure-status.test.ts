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
import { PENDING_TITLE_PLACEHOLDER, type ThreadState } from '../thread-events';
import { fetchThreads, fetchThreadById } from '../../api/threads';
import { getThreadQueue } from '../../api/client';
import { drawerOpen } from '../../components/layout/drawerState';
import { _resetComposeDraftsForTesting } from '../composeDrafts';
import { awaitedThreadId, connectionStatus, databaseReachable, focusedThreadId, generatedTitleIds, mobileView, threadDrawerOpen, threadMap, threadsLoaded, toasts, THREAD_EVENTS_LOAD_TOAST_KEY, THREAD_EVENTS_REFRESH_TOAST_KEY } from '../store';
import { _resetThreadEventsFailuresForTesting, clearThreadFetchGuards, ensureThreadByIdInMap, ensureThreadInMap, loadAllThreads, upsertThread } from './thread-loading';
import { focusSpawnedThread, focusThread, focusThreadOrBootstrap, focusThreadOrBootstrapResult, unfocusThread } from './threads';
import { _resetRowIntentsForTesting } from '../rowIntents';

vi.mock('../../api/threads', () => ({
  fetchThreads: vi.fn(),
  fetchThreadById: vi.fn(),
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
  putComposeOnThread: vi.fn().mockResolvedValue({ status: 'applied' }),
  ensureThreadStarted: vi.fn().mockResolvedValue(undefined),
  deleteThread: vi.fn().mockResolvedValue(undefined),
  getThreadQueue: vi.fn(),
}));


beforeEach(() => {
  threadMap.value = new Map();
  _resetComposeDraftsForTesting();
  focusedThreadId.value = null;
  mobileView.value = 'thread';
  threadDrawerOpen.value = false;
  drawerOpen.value = false;
  generatedTitleIds.clear();
  _resetRowIntentsForTesting();
  localStorage.removeItem('lucidos-focused-thread');
});

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
      awaited_children_count: 2,
      total_children_count: 0,
      blocking_descendant_count: 0, attention_descendant_count: 0, live_event_wait_count: 0,
      status: 'idle',
      summary_version: 0,
      coding_agent_change_state: { kind: 'none' },
      coding_agent_is_external_repo: false,
      last_revived_at: null,
      state: 'active',
      compose_text: '',
      compose_images: [],
    });

    const thread = threadMap.value.get('search-t1');
    expect(thread).toBeDefined();
    expect(thread!.meta.title).toBe('Search Result Thread');
    expect(thread!.meta.channel).toBe('chat');
    expect(thread!.meta.messageCount).toBe(3);
    expect(thread!.meta.awaitedChildrenCount).toBe(2);
    expect(thread!.eventsLoaded).toBe(true);
  });

  it('does not overwrite a thread already in the map', async () => {
    const map = new Map<string, ThreadState>();
    map.set('t1', makeThreadState('t1', {
      meta: { id: 't1', title: 'Existing Title', channel: 'claude_code', initiator: 'user', saved: true, createdAt: '', updatedAt: '2026-01-01T00:00:00Z', status: 'idle', codingAgentChangeState: { kind: 'none' }, codingAgentIsExternalRepo: false, lastRevivedAt: '', messageCount: 5, section: 'inbox', activeChildrenCount: 0 },
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
      blocking_descendant_count: 0, attention_descendant_count: 0, live_event_wait_count: 0,
      status: 'idle',
      summary_version: 0,
      coding_agent_change_state: { kind: 'none' },
      coding_agent_is_external_repo: false,
      last_revived_at: null,
      state: 'active',
      compose_text: '',
      compose_images: [],
    });

    const thread = threadMap.value.get('t1')!;
    expect(thread.meta.title).toBe('Existing Title');
    expect(thread.meta.saved).toBe(true);
  });
});


describe('ensureThreadByIdInMap', () => {
  function summary(overrides: Record<string, unknown> = {}) {
    return {
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
      summary_version: 0,
      coding_agent_change_state: { kind: 'none' },
      coding_agent_is_external_repo: false,
      last_revived_at: null,
      ...overrides,
    };
  }

  beforeEach(() => {
    (fetchThreads as any).mockClear();
    (fetchThreadById as any).mockReset();
  });

  it('fetches metadata for a thread not in the map and adds it', async () => {
    threadsLoaded.value = true;
    threadMap.value = new Map();

    (fetchThreadById as any).mockResolvedValue(summary());

    const ok = await ensureThreadByIdInMap('old-archived-1');
    expect(ok).toBe(true);
    expect(fetchThreadById).toHaveBeenCalledWith('old-archived-1');
    const added = threadMap.value.get('old-archived-1');
    expect(added).toBeDefined();
    expect(added!.meta.title).toBe('Old Archived Thread');
    expect(added!.meta.channel).toBe('chat');
  });

  it('reads the by-id endpoint, NEVER the grouped thread list', async () => {
    threadMap.value = new Map();
    (fetchThreadById as any).mockResolvedValue(summary());

    await ensureThreadByIdInMap('old-archived-1');

    expect(fetchThreads).not.toHaveBeenCalled();
  });

  it('carries `saved` off the summary, so a saved thread does not land unsaved', async () => {
    threadMap.value = new Map();
    (fetchThreadById as any).mockResolvedValue(summary({ saved: true }));

    await ensureThreadByIdInMap('old-archived-1');

    expect(threadMap.value.get('old-archived-1')!.meta.saved).toBe(true);
  });

  it('returns true without fetching when the thread is already in the map', async () => {
    const map = new Map<string, ThreadState>();
    map.set('already-here', makeThreadState('already-here', {
      meta: { id: 'already-here', title: 'Already Here', channel: 'chat', initiator: 'user', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:00Z', status: 'idle', codingAgentChangeState: { kind: 'none' }, codingAgentIsExternalRepo: false, lastRevivedAt: '', messageCount: 1, section: 'inbox', activeChildrenCount: 0 },
    }));
    threadMap.value = map;

    const ok = await ensureThreadByIdInMap('already-here');
    expect(ok).toBe(true);
    expect(fetchThreadById).not.toHaveBeenCalled();
  });

  it('returns false when the API has no record of the thread (404)', async () => {
    threadMap.value = new Map();
    (fetchThreadById as any).mockResolvedValue(null);

    const ok = await ensureThreadByIdInMap('does-not-exist');
    expect(ok).toBe(false);
    expect(threadMap.value.has('does-not-exist')).toBe(false);
  });

  it('propagates fetch errors to the caller (no swallow)', async () => {
    threadMap.value = new Map();
    (fetchThreadById as any).mockRejectedValue(new Error('network down'));

    await expect(ensureThreadByIdInMap('any-id')).rejects.toThrow('network down');
  });
});


describe('focusThreadOrBootstrapResult, optimistic focus while bootstrapping', () => {
  function summary(id: string) {
    return {
      thread_id: id,
      title: 'Bootstrapped',
      channel: 'chat',
      initiator: 'user',
      last_activity: '2026-01-01T00:00:00Z',
      created_at: '2026-01-01T00:00:00Z',
      message_count: 1,
      section: 'archived',
      active_children_count: 0,
      total_children_count: 0,
      status: 'idle',
      summary_version: 0,
      coding_agent_change_state: { kind: 'none' },
      coding_agent_is_external_repo: false,
      last_revived_at: null,
    };
  }

  function deferred<T>(): { promise: Promise<T>; resolve: (v: T) => void; reject: (e: unknown) => void } {
    let resolve!: (v: T) => void;
    let reject!: (e: unknown) => void;
    const promise = new Promise<T>((res, rej) => { resolve = res; reject = rej; });
    return { promise, resolve, reject };
  }

  beforeEach(() => {
    threadsLoaded.value = true;
    threadMap.value = new Map();
    awaitedThreadId.value = null;
    (fetchThreadById as any).mockReset();
    vi.mocked(getThreadQueue).mockResolvedValue({ entries: [], policy: {} as any });
  });

  it('focuses the target and flags the bootstrap BEFORE the metadata arrives', async () => {
    const d = deferred<any>();
    (fetchThreadById as any).mockReturnValue(d.promise);

    const pending = focusThreadOrBootstrapResult('target');

    expect(focusedThreadId.value).toBe('target');
    expect(awaitedThreadId.value).toBe('target');

    d.resolve(summary('target'));
    await pending;
  });

  it('clears the bootstrap flag once the thread is really in the map', async () => {
    (fetchThreadById as any).mockResolvedValue(summary('target'));

    const outcome = await focusThreadOrBootstrapResult('target');

    expect(outcome.kind).toBe('focused');
    expect(focusedThreadId.value).toBe('target');
    expect(awaitedThreadId.value).toBeNull();
  });

  it('restores the previous focus when the thread does not exist', async () => {
    threadMap.value = new Map([['was-here', makeThreadState('was-here')]]);
    focusedThreadId.value = 'was-here';
    (fetchThreadById as any).mockResolvedValue(null);

    const outcome = await focusThreadOrBootstrapResult('ghost');

    expect(outcome.kind).toBe('not-found');
    expect(focusedThreadId.value).toBe('was-here');
    expect(awaitedThreadId.value).toBeNull();
  });

  it('holds the focus on a spawn the thread queue still holds', async () => {
    threadMap.value = new Map([['was-here', makeThreadState('was-here')]]);
    focusedThreadId.value = 'was-here';
    (fetchThreadById as any).mockResolvedValue(null);
    vi.mocked(getThreadQueue).mockResolvedValue({
      entries: [{ id: 'e1', kind: 'coding-agent', thread_id: 'queued', summary: 'Next', status: 'queued', queued_at: '2026-01-01T00:00:00Z' }],
      policy: {} as any,
    });
    toasts.value = [];

    const outcome = await focusThreadOrBootstrapResult('queued');
    focusThreadOrBootstrap('queued');
    await new Promise((r) => setTimeout(r, 0));

    expect(outcome.kind).toBe('awaiting');
    expect(focusedThreadId.value).toBe('queued');
    expect(awaitedThreadId.value).toBe('queued');
    expect(toasts.value).toEqual([]);
  });

  it('focuses a spawn the queue admitted between the two reads', async () => {
    threadMap.value = new Map([['was-here', makeThreadState('was-here')]]);
    focusedThreadId.value = 'was-here';
    (fetchThreadById as any).mockResolvedValueOnce(null).mockResolvedValue(summary('admitted'));
    vi.mocked(getThreadQueue).mockResolvedValue({ entries: [], policy: {} as any });

    const outcome = await focusThreadOrBootstrapResult('admitted');

    expect(outcome.kind).toBe('focused');
    expect(focusedThreadId.value).toBe('admitted');
    expect(awaitedThreadId.value).toBeNull();
  });

  it('reports a failed queue read as a failure, not as a missing thread', async () => {
    threadMap.value = new Map([['was-here', makeThreadState('was-here')]]);
    focusedThreadId.value = 'was-here';
    (fetchThreadById as any).mockResolvedValue(null);
    vi.mocked(getThreadQueue).mockRejectedValue(new Error('network down'));

    const outcome = await focusThreadOrBootstrapResult('unknown');

    expect(outcome.kind).toBe('failed');
    expect(focusedThreadId.value).toBe('was-here');
    expect(awaitedThreadId.value).toBeNull();
  });

  it('names the thread and the origin when the bootstrap misses', async () => {
    (fetchThreadById as any).mockResolvedValue(null);
    toasts.value = [];

    focusThreadOrBootstrap('ghost-1', { source: 'thread "Nightly build"' });
    await new Promise((r) => setTimeout(r, 0));

    const message = toasts.value.map((t) => t.message).join(' ');
    expect(message).toContain('ghost-1');
    expect(message).toContain('requested by thread "Nightly build"');
  });

  it('names the thread even when nothing said where the navigate came from', async () => {
    (fetchThreadById as any).mockResolvedValue(null);
    toasts.value = [];

    focusThreadOrBootstrap('ghost-2');
    await new Promise((r) => setTimeout(r, 0));

    const message = toasts.value.map((t) => t.message).join(' ');
    expect(message).toContain('ghost-2');
    expect(message).not.toContain('requested by');
  });

  it('restores the previous focus when the fetch fails', async () => {
    threadMap.value = new Map([['was-here', makeThreadState('was-here')]]);
    focusedThreadId.value = 'was-here';
    (fetchThreadById as any).mockRejectedValue(new Error('network down'));

    const outcome = await focusThreadOrBootstrapResult('unreachable');

    expect(outcome.kind).toBe('failed');
    expect(focusedThreadId.value).toBe('was-here');
    expect(awaitedThreadId.value).toBeNull();
  });

  it('a superseded bootstrap does not yank focus off the newer one', async () => {
    const first = deferred<any>();
    const second = deferred<any>();
    (fetchThreadById as any)
      .mockReturnValueOnce(first.promise)
      .mockReturnValueOnce(second.promise);

    const p1 = focusThreadOrBootstrapResult('first');
    const p2 = focusThreadOrBootstrapResult('second');
    expect(awaitedThreadId.value).toBe('second');

    first.reject(new Error('too late'));
    await p1;

    expect(focusedThreadId.value).toBe('second');
    expect(awaitedThreadId.value).toBe('second');

    second.resolve(summary('second'));
    await p2;
    expect(focusedThreadId.value).toBe('second');
    expect(awaitedThreadId.value).toBeNull();
  });

  it('a thread already in the map focuses synchronously and never flags a bootstrap', () => {
    threadMap.value = new Map([['warm', makeThreadState('warm')]]);

    void focusThreadOrBootstrapResult('warm');

    expect(focusedThreadId.value).toBe('warm');
    expect(awaitedThreadId.value).toBeNull();
    expect(fetchThreadById).not.toHaveBeenCalled();
  });

  it('focusSpawnedThread claims the await for a thread not in the map', () => {
    focusSpawnedThread('spawned');

    expect(focusedThreadId.value).toBe('spawned');
    expect(awaitedThreadId.value).toBe('spawned');
    expect(threadsLoaded.value && awaitedThreadId.value !== 'spawned').toBe(false);
  });

  it('moving the focus off an awaited thread releases the await', () => {
    focusSpawnedThread('spawned');
    threadMap.value = new Map([['elsewhere', makeThreadState('elsewhere')]]);

    focusThread('elsewhere');

    expect(awaitedThreadId.value).toBeNull();
  });

  it('unfocusing releases the await too', () => {
    focusSpawnedThread('spawned');

    unfocusThread();

    expect(focusedThreadId.value).toBeNull();
    expect(awaitedThreadId.value).toBeNull();
  });
});


describe('CC thread spawned by chat — status from API is authoritative', () => {
  it('API status always overwrites SSE skeleton status', async () => {
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
        summaryVersion: 0,
      codingAgentChangeState: { kind: 'none' },
      codingAgentIsExternalRepo: false,
        lastRevivedAt: '',
        messageCount: 0,
        section: 'archived',
        activeChildrenCount: 0,
      },
    }));
    threadMap.value = map;

    (fetchThreads as any).mockResolvedValue({
      saved: [],
      active_threads: [],
      composing: [],
      active: [],
      archive: [
        { thread_id: 'cc-1', title: 'Fix OAuth URLs', channel: 'claude_code', last_activity: '2026-03-19T20:00:00Z', created_at: '2026-03-19T20:00:00Z', message_count: 1, section: 'archived', active_children_count: 0, status: 'idle', coding_agent_change_state: { kind: 'none' }, coding_agent_is_external_repo: false, last_revived_at: null },
      ],
    });

    await loadAllThreads();

    expect(threadMap.value.get('cc-1')!.meta.status).toBe('idle');
  });

  it('API status is applied even if SSE status differs', async () => {
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
        summaryVersion: 0,
      codingAgentChangeState: { kind: 'none' },
      codingAgentIsExternalRepo: false,
        lastRevivedAt: '',
        messageCount: 0,
        section: 'archived',
        activeChildrenCount: 0,
      },
      eventsLoaded: true,
    }));
    threadMap.value = map;

    (fetchThreads as any).mockResolvedValue({
      saved: [],
      active_threads: [],
      composing: [],
      active: [],
      archive: [
        { thread_id: 'cc-1', title: 'Aborted Session', channel: 'claude_code', last_activity: '2026-03-19T20:00:00Z', created_at: '2026-03-19T20:00:00Z', message_count: 1, section: 'inbox', status: 'idle', coding_agent_change_state: { kind: 'none' }, coding_agent_is_external_repo: false, last_revived_at: null, active_children_count: 0 },
      ],
    });

    await loadAllThreads();

    expect(threadMap.value.get('cc-1')!.meta.status).toBe('idle');
  });
});


describe('event replay must not override API status', () => {
  it('API status=idle is preserved after replaying events ending with CodingAgentPromptSent', async () => {
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
          status: 'idle',
          coding_agent_change_state: { kind: 'none' },
          coding_agent_is_external_repo: false,
          last_revived_at: null,
        },
      ],
      active: ['stuck-t1'],
      archive: [],
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
    ], currentAggregate: {
      threadId: 'stuck-t1',
      title: 'Stuck Thread',
      channel: 'claude_code',
      initiator: 'user',
      createdAt: '2026-03-28T19:00:00Z',
      lastActivity: '2026-03-30T08:41:01Z',
      messageCount: 5,
      section: 'inbox',
      status: 'idle',
      summaryVersion: 0,
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
      latestTodoList: null,
      liveEventWaitCount: 0,
      liveEventWaits: [],
    } });

    await loadAllThreads();

    const thread = threadMap.value.get('stuck-t1')!;
    expect(thread.meta.status).toBe('idle');
  });

  it('refreshThreadEvents allows new events to update status', async () => {
    const map = new Map<string, ThreadState>();
    map.set('refresh-t1', makeThreadState('refresh-t1', {
      meta: { id: 'refresh-t1', title: 'Refresh Thread', channel: 'claude_code', saved: false, createdAt: '2026-03-28T19:00:00Z', updatedAt: '2026-03-28T19:00:00Z', status: 'idle', codingAgentChangeState: { kind: 'none' }, codingAgentIsExternalRepo: false, lastRevivedAt: '', messageCount: 1, section: 'archived', activeChildrenCount: 0 },
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
      threadId: 'refresh-t1',
      title: 'Refresh Thread',
      channel: 'claude_code',
      initiator: 'user',
      createdAt: '2026-03-28T19:00:00Z',
      lastActivity: '2026-03-30T10:00:00Z',
      messageCount: 2,
      section: 'archived',
      status: 'running',
      summaryVersion: 0,
      activeChildrenCount: 0,
      totalChildrenCount: 0,
      blockingDescendantCount: 0, attentionDescendantCount: 0,
      codingAgentChangeState: { kind: 'none' },
      codingAgentIsExternalRepo: false,
      isSaved: false,
      hasResponse: false,
      lastRevivedAt: '2026-03-30T10:00:00Z',
      parentThreadId: null,
      parentThreadTitle: null,
      state: 'active',
      latestTodoList: null,
      liveEventWaitCount: 0,
      liveEventWaits: [],
    } });

    const { refreshThreadEvents } = await import('./thread-loading');
    await refreshThreadEvents('refresh-t1');

    expect(threadMap.value.get('refresh-t1')!.meta.status).toBe('running');
  });
});


describe('refreshThreadEvents — retry on transient DOMException / transport error', () => {
  beforeEach(() => {
    toasts.value = [];
    _resetThreadEventsFailuresForTesting();
    clearThreadFetchGuards();
    connectionStatus.value = 'connected';
    databaseReachable.value = true;
  });

  function setupLoadedThread(): void {
    const map = new Map<string, ThreadState>();
    map.set('t1', makeThreadState('t1', {
      meta: { id: 't1', title: 'Refresh Retry Thread' },
      eventsLoaded: true,
      lastDbSeq: 3,
    }));
    threadMap.value = map;
  }

  function noNewEventsSnapshot(): { events: never[]; currentAggregate: null } {
    return { events: [], currentAggregate: null };
  }

  it('retries silently when the first fetch aborts (iOS suspend/resume) and second succeeds', async () => {
    setupLoadedThread();
    const { fetchThreadEvents } = await import('../../api/threads');
    const mock = fetchThreadEvents as unknown as ReturnType<typeof vi.fn>;
    mock.mockReset();
    mock
      .mockRejectedValueOnce(new DOMException('aborted', 'AbortError'))
      .mockResolvedValueOnce(noNewEventsSnapshot());

    const { refreshThreadEvents } = await import('./thread-loading');
    await refreshThreadEvents('t1');

    expect(mock).toHaveBeenCalledTimes(2);
    expect(toasts.value.find(t => t.message?.startsWith('Failed to refresh thread events'))).toBeUndefined();
  });

  it('retries silently when the first fetch times out and second succeeds', async () => {
    setupLoadedThread();
    const { fetchThreadEvents } = await import('../../api/threads');
    const mock = fetchThreadEvents as unknown as ReturnType<typeof vi.fn>;
    mock.mockReset();
    mock
      .mockRejectedValueOnce(new DOMException('timeout', 'TimeoutError'))
      .mockResolvedValueOnce(noNewEventsSnapshot());

    const { refreshThreadEvents } = await import('./thread-loading');
    await refreshThreadEvents('t1');

    expect(mock).toHaveBeenCalledTimes(2);
    expect(toasts.value.find(t => t.message?.startsWith('Failed to refresh thread events'))).toBeUndefined();
  });

  it('silences both-aborted (PWA wake) per the frontend.md best-effort-telemetry carve-out', async () => {
    setupLoadedThread();
    const { fetchThreadEvents } = await import('../../api/threads');
    const mock = fetchThreadEvents as unknown as ReturnType<typeof vi.fn>;
    mock.mockReset();
    mock
      .mockRejectedValueOnce(new DOMException('aborted', 'AbortError'))
      .mockRejectedValueOnce(new DOMException('aborted', 'AbortError'));

    const { refreshThreadEvents } = await import('./thread-loading');
    await refreshThreadEvents('t1');

    expect(mock).toHaveBeenCalledTimes(2);
    expect(toasts.value.find(t => t.message?.startsWith('Failed to refresh thread events'))).toBeUndefined();
  });

  it('retries silently when the first fetch fails with a transport error (iOS stale connection) and second succeeds', async () => {
    setupLoadedThread();
    const { fetchThreadEvents } = await import('../../api/threads');
    const mock = fetchThreadEvents as unknown as ReturnType<typeof vi.fn>;
    mock.mockReset();
    mock
      .mockRejectedValueOnce(new TypeError('Load failed'))
      .mockResolvedValueOnce(noNewEventsSnapshot());

    const { refreshThreadEvents } = await import('./thread-loading');
    await refreshThreadEvents('t1');

    expect(mock).toHaveBeenCalledTimes(2);
    expect(toasts.value.find(t => t.message?.startsWith('Failed to refresh thread events'))).toBeUndefined();
  });

  it('silences both-transport-error (iOS PWA wake / post-restart stale HTTP/2) per the carve-out', async () => {
    setupLoadedThread();
    const { fetchThreadEvents } = await import('../../api/threads');
    const mock = fetchThreadEvents as unknown as ReturnType<typeof vi.fn>;
    mock.mockReset();
    mock
      .mockRejectedValueOnce(new TypeError('Load failed'))
      .mockRejectedValueOnce(new TypeError('Load failed'));

    const { refreshThreadEvents } = await import('./thread-loading');
    await refreshThreadEvents('t1');

    expect(mock).toHaveBeenCalledTimes(2);
    expect(toasts.value.find(t => t.message?.startsWith('Failed to refresh thread events'))).toBeUndefined();
  });

  it('silences both-timed-out, the exact card the user reported', async () => {
    setupLoadedThread();
    const { fetchThreadEvents } = await import('../../api/threads');
    const mock = fetchThreadEvents as unknown as ReturnType<typeof vi.fn>;
    mock.mockReset();
    mock
      .mockRejectedValueOnce(new DOMException('timeout', 'TimeoutError'))
      .mockRejectedValueOnce(new DOMException('timeout', 'TimeoutError'));

    const { refreshThreadEvents } = await import('./thread-loading');
    await refreshThreadEvents('t1');

    expect(mock).toHaveBeenCalledTimes(2);
    expect(toasts.value.find(t => t.message?.startsWith('Failed to refresh thread events'))).toBeUndefined();
  });

  it('surfaces a verdict immediately (no retry) under the shared key', async () => {
    setupLoadedThread();
    const { fetchThreadEvents } = await import('../../api/threads');
    const mock = fetchThreadEvents as unknown as ReturnType<typeof vi.fn>;
    mock.mockReset();
    mock.mockRejectedValueOnce(new Error('boom'));

    const { refreshThreadEvents } = await import('./thread-loading');
    await refreshThreadEvents('t1');

    expect(mock).toHaveBeenCalledTimes(1);
    const toast = toasts.value.find(t => t.message?.startsWith('Failed to refresh thread events'));
    expect(toast!.message).toBe('Failed to refresh thread events for "Refresh Retry Thread": boom');
    expect(toast!.type).toBe('error');
    expect(toast!.key).toBe(THREAD_EVENTS_REFRESH_TOAST_KEY);
  });

  it('reports ten verdict failures as ONE card that counts them', async () => {
    const map = new Map<string, ThreadState>();
    for (let i = 0; i < 10; i++) {
      map.set(`v${i}`, makeThreadState(`v${i}`, {
        meta: { id: `v${i}`, title: `Verdict Thread ${i}` },
        eventsLoaded: true,
        lastDbSeq: 1,
      }));
    }
    threadMap.value = map;
    const { fetchThreadEvents } = await import('../../api/threads');
    const mock = fetchThreadEvents as unknown as ReturnType<typeof vi.fn>;
    mock.mockReset();
    mock.mockRejectedValue(new Error('boom'));

    const { refreshThreadEvents } = await import('./thread-loading');
    await Promise.all([...map.keys()].map(id => refreshThreadEvents(id)));

    expect(toasts.value).toHaveLength(1);
    expect(toasts.value[0].message).toBe('Failed to refresh thread events for 10 threads: boom');
  });

  it('counts a thread with no title yet rather than falling back to its id', async () => {
    const map = new Map<string, ThreadState>();
    map.set('untitled', makeThreadState('untitled', {
      meta: { id: 'untitled', title: PENDING_TITLE_PLACEHOLDER },
      eventsLoaded: true,
      lastDbSeq: 1,
    }));
    threadMap.value = map;
    const { fetchThreadEvents } = await import('../../api/threads');
    const mock = fetchThreadEvents as unknown as ReturnType<typeof vi.fn>;
    mock.mockReset();
    mock.mockRejectedValue(new Error('boom'));

    const { refreshThreadEvents } = await import('./thread-loading');
    await refreshThreadEvents('untitled');

    expect(toasts.value[0].message).toBe('Failed to refresh thread events for 1 thread: boom');
    expect(toasts.value[0].message).not.toContain('untitled');
  });

  it('records nothing and shows no card while the engine is unreachable', async () => {
    setupLoadedThread();
    connectionStatus.value = 'disconnected';
    const { fetchThreadEvents } = await import('../../api/threads');
    const mock = fetchThreadEvents as unknown as ReturnType<typeof vi.fn>;
    mock.mockReset();
    mock.mockRejectedValue(new Error('boom'));

    const { refreshThreadEvents } = await import('./thread-loading');
    await refreshThreadEvents('t1');

    expect(toasts.value).toHaveLength(0);
  });

  it('a transient failure never joins the failing set, so a later verdict counts only itself', async () => {
    const map = new Map<string, ThreadState>();
    map.set('blip', makeThreadState('blip', {
      meta: { id: 'blip', title: 'Blipped Thread' }, eventsLoaded: true, lastDbSeq: 1,
    }));
    map.set('real', makeThreadState('real', {
      meta: { id: 'real', title: 'Refused Thread' }, eventsLoaded: true, lastDbSeq: 1,
    }));
    threadMap.value = map;
    const { fetchThreadEvents } = await import('../../api/threads');
    const mock = fetchThreadEvents as unknown as ReturnType<typeof vi.fn>;
    mock.mockReset();
    mock.mockImplementation((id: string) => id === 'blip'
      ? Promise.reject(new DOMException('timeout', 'TimeoutError'))
      : Promise.reject(new Error('boom')));

    const { refreshThreadEvents } = await import('./thread-loading');
    await refreshThreadEvents('blip');
    await refreshThreadEvents('real');

    expect(toasts.value).toHaveLength(1);
    expect(toasts.value[0].message).toBe('Failed to refresh thread events for "Refused Thread": boom');
  });

  it('recounts on a partial recovery instead of freezing the count it was raised with', async () => {
    const map = new Map<string, ThreadState>();
    for (const id of ['a', 'b', 'c']) {
      map.set(id, makeThreadState(id, {
        meta: { id, title: `Thread ${id.toUpperCase()}` }, eventsLoaded: true, lastDbSeq: 1,
      }));
    }
    threadMap.value = map;
    const { fetchThreadEvents } = await import('../../api/threads');
    const mock = fetchThreadEvents as unknown as ReturnType<typeof vi.fn>;
    mock.mockReset();
    mock.mockRejectedValue(new Error('boom'));

    const { refreshThreadEvents } = await import('./thread-loading');
    await refreshThreadEvents('a');
    await refreshThreadEvents('b');
    await refreshThreadEvents('c');
    expect(toasts.value).toHaveLength(1);
    expect(toasts.value[0].message).toBe('Failed to refresh thread events for 3 threads: boom');

    mock.mockReset();
    mock.mockResolvedValue(noNewEventsSnapshot());
    await refreshThreadEvents('a');
    expect(toasts.value).toHaveLength(1);
    expect(toasts.value[0].message).toBe('Failed to refresh thread events for 2 threads: boom');

    await refreshThreadEvents('b');
    expect(toasts.value[0].message).toBe('Failed to refresh thread events for "Thread C": boom');

    await refreshThreadEvents('c');
    expect(toasts.value).toHaveLength(0);
  });

  it('coalesces two fan-out refreshes of the same thread, so a slow failure cannot outlive a fast success', async () => {
    setupLoadedThread();
    const { fetchThreadEvents } = await import('../../api/threads');
    const mock = fetchThreadEvents as unknown as ReturnType<typeof vi.fn>;
    mock.mockReset();
    let settleFirst: (v: unknown) => void = () => {};
    mock
      .mockImplementationOnce(() => new Promise(res => { settleFirst = res; }))
      .mockRejectedValue(new Error('boom'));

    const { refreshThreadEvents } = await import('./thread-loading');
    const first = refreshThreadEvents('t1', { coalesce: true });
    const second = refreshThreadEvents('t1', { coalesce: true });
    settleFirst(noNewEventsSnapshot());
    await Promise.all([first, second]);

    expect(mock).toHaveBeenCalledTimes(1);
    expect(toasts.value).toHaveLength(0);
  });

  it('a read-after-write caller always issues its own request, even alongside a fan-out', async () => {
    setupLoadedThread();
    const { fetchThreadEvents } = await import('../../api/threads');
    const mock = fetchThreadEvents as unknown as ReturnType<typeof vi.fn>;
    mock.mockReset();
    let settleFanOut: (v: unknown) => void = () => {};
    mock
      .mockImplementationOnce(() => new Promise(res => { settleFanOut = res; }))
      .mockResolvedValue(noNewEventsSnapshot());

    const { refreshThreadEvents } = await import('./thread-loading');
    const fanOut = refreshThreadEvents('t1', { coalesce: true });
    await refreshThreadEvents('t1');
    expect(mock).toHaveBeenCalledTimes(2);

    settleFanOut(noNewEventsSnapshot());
    await fanOut;
  });

  it('an attempt whose guard was reset mid-flight does not release the newer attempt', async () => {
    setupLoadedThread();
    const { fetchThreadEvents } = await import('../../api/threads');
    const mock = fetchThreadEvents as unknown as ReturnType<typeof vi.fn>;
    mock.mockReset();
    const settle: Array<(v: unknown) => void> = [];
    mock.mockImplementation(() => new Promise(res => { settle.push(res); }));

    const { refreshThreadEvents } = await import('./thread-loading');
    const first = refreshThreadEvents('t1', { coalesce: true });
    clearThreadFetchGuards();
    const second = refreshThreadEvents('t1', { coalesce: true });
    expect(mock).toHaveBeenCalledTimes(2);

    settle[0](noNewEventsSnapshot());
    await first;
    await refreshThreadEvents('t1', { coalesce: true });
    expect(mock).toHaveBeenCalledTimes(2);

    settle[1](noNewEventsSnapshot());
    await second;
    const third = refreshThreadEvents('t1', { coalesce: true });
    expect(mock).toHaveBeenCalledTimes(3);
    settle[2](noNewEventsSnapshot());
    await third;
  });

  it('a superseded attempt reports nothing, whichever way it settles', async () => {
    setupLoadedThread();
    const { fetchThreadEvents } = await import('../../api/threads');
    const mock = fetchThreadEvents as unknown as ReturnType<typeof vi.fn>;
    const { refreshThreadEvents } = await import('./thread-loading');

    mock.mockReset();
    let failOld: (e: unknown) => void = () => {};
    mock.mockImplementationOnce(() => new Promise((_res, rej) => { failOld = rej; }))
      .mockResolvedValue(noNewEventsSnapshot());
    const old1 = refreshThreadEvents('t1', { coalesce: true });
    clearThreadFetchGuards();
    await refreshThreadEvents('t1', { coalesce: true });
    failOld(new Error('boom'));
    await old1;
    expect(toasts.value).toHaveLength(0);

    _resetThreadEventsFailuresForTesting();
    clearThreadFetchGuards();
    toasts.value = [];
    mock.mockReset();
    let succeedOld: (v: unknown) => void = () => {};
    mock.mockImplementationOnce(() => new Promise(res => { succeedOld = res; }))
      .mockRejectedValue(new Error('boom'));
    const old2 = refreshThreadEvents('t1', { coalesce: true });
    clearThreadFetchGuards();
    await refreshThreadEvents('t1', { coalesce: true });
    expect(toasts.value).toHaveLength(1);
    succeedOld(noNewEventsSnapshot());
    await old2;
    expect(toasts.value).toHaveLength(1);
  });

  it('still reports a verdict when the newer attempt settled without concluding anything', async () => {
    setupLoadedThread();
    const { fetchThreadEvents } = await import('../../api/threads');
    const mock = fetchThreadEvents as unknown as ReturnType<typeof vi.fn>;
    mock.mockReset();
    let failOld: (e: unknown) => void = () => {};
    mock.mockImplementationOnce(() => new Promise((_res, rej) => { failOld = rej; }))
      .mockRejectedValue(new DOMException('aborted', 'AbortError'));

    const { refreshThreadEvents } = await import('./thread-loading');
    const older = refreshThreadEvents('t1', { coalesce: true });
    clearThreadFetchGuards();
    await refreshThreadEvents('t1', { coalesce: true });
    failOld(new Error('boom'));
    await older;

    expect(toasts.value).toHaveLength(1);
    expect(toasts.value[0].message).toContain('boom');
  });

  it('a recovery never RAISES a card that was suppressed while the failures were real', async () => {
    const map = new Map<string, ThreadState>();
    for (const id of ['x', 'y', 'z']) {
      map.set(id, makeThreadState(id, { meta: { id, title: id }, eventsLoaded: true, lastDbSeq: 1 }));
    }
    threadMap.value = map;
    databaseReachable.value = false;
    const { fetchThreadEvents } = await import('../../api/threads');
    const mock = fetchThreadEvents as unknown as ReturnType<typeof vi.fn>;
    mock.mockReset();
    mock.mockRejectedValue(new Error('database unreachable'));

    const { refreshThreadEvents } = await import('./thread-loading');
    for (const id of ['x', 'y', 'z']) await refreshThreadEvents(id);
    expect(toasts.value).toHaveLength(0);

    databaseReachable.value = true;
    mock.mockReset();
    mock.mockResolvedValue(noNewEventsSnapshot());
    await refreshThreadEvents('x');

    expect(toasts.value).toHaveLength(0);
    await refreshThreadEvents('y');
    await refreshThreadEvents('z');
    expect(toasts.value).toHaveLength(0);
  });

  it('shows the newest reason when a thread fails again with a different one', async () => {
    const map = new Map<string, ThreadState>();
    map.set('p', makeThreadState('p', { meta: { id: 'p', title: 'P' }, eventsLoaded: true, lastDbSeq: 1 }));
    map.set('q', makeThreadState('q', { meta: { id: 'q', title: 'Q' }, eventsLoaded: true, lastDbSeq: 1 }));
    threadMap.value = map;
    const { fetchThreadEvents } = await import('../../api/threads');
    const mock = fetchThreadEvents as unknown as ReturnType<typeof vi.fn>;
    mock.mockReset();

    const { refreshThreadEvents } = await import('./thread-loading');
    mock.mockRejectedValue(new Error('boom'));
    await refreshThreadEvents('p');
    mock.mockRejectedValue(new Error('500 Internal Server Error'));
    await refreshThreadEvents('q');
    mock.mockRejectedValue(new Error('connection reset'));
    await refreshThreadEvents('p');

    expect(toasts.value[0].message).toBe('Failed to refresh thread events for 2 threads: connection reset');
  });

  it('retracts a lone failing thread the moment it is removed, with no later fetch to prune it', async () => {
    setupLoadedThread();
    const { fetchThreadEvents } = await import('../../api/threads');
    const mock = fetchThreadEvents as unknown as ReturnType<typeof vi.fn>;
    mock.mockReset();
    mock.mockRejectedValue(new Error('boom'));

    const { refreshThreadEvents, forgetThreadEventsFailures } = await import('./thread-loading');
    await refreshThreadEvents('t1');
    expect(toasts.value).toHaveLength(1);

    const pruned = new Map(threadMap.value);
    pruned.delete('t1');
    threadMap.value = pruned;
    forgetThreadEventsFailures('t1');

    expect(toasts.value).toHaveLength(0);
  });

  it('drops a thread that leaves the map, so its card cannot outlive it', async () => {
    const map = new Map<string, ThreadState>();
    map.set('doomed', makeThreadState('doomed', {
      meta: { id: 'doomed', title: 'Rolled Back' }, eventsLoaded: true, lastDbSeq: 1,
    }));
    map.set('kept', makeThreadState('kept', {
      meta: { id: 'kept', title: 'Kept' }, eventsLoaded: true, lastDbSeq: 1,
    }));
    threadMap.value = map;
    const { fetchThreadEvents } = await import('../../api/threads');
    const mock = fetchThreadEvents as unknown as ReturnType<typeof vi.fn>;
    mock.mockReset();
    mock.mockRejectedValue(new Error('boom'));

    const { refreshThreadEvents } = await import('./thread-loading');
    await refreshThreadEvents('doomed');
    await refreshThreadEvents('kept');
    expect(toasts.value[0].message).toBe('Failed to refresh thread events for 2 threads: boom');

    const pruned = new Map(threadMap.value);
    pruned.delete('doomed');
    threadMap.value = pruned;

    mock.mockReset();
    mock.mockResolvedValue(noNewEventsSnapshot());
    await refreshThreadEvents('kept');

    expect(toasts.value).toHaveLength(0);
  });

  it('does not toast or retry on success', async () => {
    setupLoadedThread();
    const { fetchThreadEvents } = await import('../../api/threads');
    const mock = fetchThreadEvents as unknown as ReturnType<typeof vi.fn>;
    mock.mockReset();
    mock.mockResolvedValueOnce(noNewEventsSnapshot());

    const { refreshThreadEvents } = await import('./thread-loading');
    await refreshThreadEvents('t1');

    expect(mock).toHaveBeenCalledTimes(1);
    expect(toasts.value.find(t => t.message?.startsWith('Failed to refresh thread events'))).toBeUndefined();
  });
});


describe('loadThreadEvents: verdict vs delivery failure', () => {
  beforeEach(() => {
    toasts.value = [];
    _resetThreadEventsFailuresForTesting();
    clearThreadFetchGuards();
    connectionStatus.value = 'connected';
    const map = new Map<string, ThreadState>();
    map.set('L1', makeThreadState('L1', { meta: { id: 'L1', title: 'Load Thread' } }));
    threadMap.value = map;
  });

  async function runLoad(threadId: string): Promise<void> {
    const { loadThreadEvents } = await import('./thread-loading');
    vi.useFakeTimers();
    try {
      const done = loadThreadEvents(threadId);
      await vi.advanceTimersByTimeAsync(5000);
      await done;
    } finally {
      vi.useRealTimers();
    }
  }

  it('stays silent when all three attempts time out, but still records the failure on the thread', async () => {
    const { fetchThreadEvents } = await import('../../api/threads');
    const mock = fetchThreadEvents as unknown as ReturnType<typeof vi.fn>;
    mock.mockReset();
    mock.mockRejectedValue(new DOMException('timeout', 'TimeoutError'));

    await runLoad('L1');

    expect(mock).toHaveBeenCalledTimes(3);
    expect(toasts.value).toHaveLength(0);
    expect(threadMap.value.get('L1')!.eventsLoadFailed).toBe(true);
  });

  it('raises one keyed card on a verdict', async () => {
    const { fetchThreadEvents } = await import('../../api/threads');
    const mock = fetchThreadEvents as unknown as ReturnType<typeof vi.fn>;
    mock.mockReset();
    mock.mockRejectedValue(new Error('boom'));

    await runLoad('L1');

    expect(toasts.value).toHaveLength(1);
    expect(toasts.value[0].message).toBe('Failed to load thread events for "Load Thread": boom');
    expect(toasts.value[0].key).toBe(THREAD_EVENTS_LOAD_TOAST_KEY);
    expect(threadMap.value.get('L1')!.eventsLoadFailed).toBe(true);
  });

  it('never leaves a load card over a thread another attempt already loaded', async () => {
    const { fetchThreadEvents } = await import('../../api/threads');
    const mock = fetchThreadEvents as unknown as ReturnType<typeof vi.fn>;
    mock.mockReset();
    mock.mockRejectedValue(new Error('boom'));

    const { loadThreadEvents } = await import('./thread-loading');
    vi.useFakeTimers();
    try {
      const failing = loadThreadEvents('L1');
      await vi.advanceTimersByTimeAsync(1500);
      threadMap.value.get('L1')!.eventsLoaded = true;
      await vi.advanceTimersByTimeAsync(5000);
      await failing;
    } finally {
      vi.useRealTimers();
    }

    expect(toasts.value).toHaveLength(0);
    expect(threadMap.value.get('L1')!.eventsLoadFailed).toBe(false);
  });

  it('a refresh that started before a full load cannot re-raise the card it retracted', async () => {
    const map = threadMap.value;
    map.set('R1', makeThreadState('R1', {
      meta: { id: 'R1', title: 'Raced Thread' }, eventsLoaded: true, lastDbSeq: 1,
    }));
    threadMap.value = new Map(map);
    const { fetchThreadEvents } = await import('../../api/threads');
    const mock = fetchThreadEvents as unknown as ReturnType<typeof vi.fn>;
    mock.mockReset();

    let failRefresh: (e: unknown) => void = () => {};
    mock.mockImplementationOnce(() => new Promise((_res, rej) => { failRefresh = rej; }));
    const { refreshThreadEvents, loadThreadEvents } = await import('./thread-loading');
    const refreshing = refreshThreadEvents('R1');

    threadMap.value.get('R1')!.eventsLoaded = false;
    mock.mockResolvedValue({ events: [], currentAggregate: null });
    await loadThreadEvents('R1');
    expect(toasts.value).toHaveLength(0);

    failRefresh(new Error('boom'));
    await refreshing;

    expect(toasts.value).toHaveLength(0);
  });

  it('a load never lowers the refresh report mark below what already reported', async () => {
    const map = threadMap.value;
    map.set('R2', makeThreadState('R2', {
      meta: { id: 'R2', title: 'Marked Thread' }, eventsLoaded: true, lastDbSeq: 1,
    }));
    threadMap.value = new Map(map);
    const { fetchThreadEvents } = await import('../../api/threads');
    const mock = fetchThreadEvents as unknown as ReturnType<typeof vi.fn>;
    mock.mockReset();

    const { refreshThreadEvents, loadThreadEvents } = await import('./thread-loading');
    let settleLoad: (v: unknown) => void = () => {};
    mock.mockImplementationOnce(() => new Promise(res => { settleLoad = res; }));
    threadMap.value.get('R2')!.eventsLoaded = false;
    const loading = loadThreadEvents('R2');
    threadMap.value.get('R2')!.eventsLoaded = true;

    let failMiddle: (e: unknown) => void = () => {};
    mock.mockImplementationOnce(() => new Promise((_res, rej) => { failMiddle = rej; }));
    const middle = refreshThreadEvents('R2');
    mock.mockRejectedValue(new Error('newest'));
    await refreshThreadEvents('R2');
    expect(toasts.value[0].message).toContain('newest');

    settleLoad({ events: [], currentAggregate: null });
    await loading;
    failMiddle(new Error('stale middle'));
    await middle;

    expect(toasts.value.some(t => t.message?.includes('stale middle'))).toBe(false);
  });

  it('a load card and a refresh card coexist, and neither retracts the other', async () => {
    const map = threadMap.value;
    map.set('R1', makeThreadState('R1', {
      meta: { id: 'R1', title: 'Refresh Thread' }, eventsLoaded: true, lastDbSeq: 1,
    }));
    threadMap.value = new Map(map);
    const { fetchThreadEvents } = await import('../../api/threads');
    const mock = fetchThreadEvents as unknown as ReturnType<typeof vi.fn>;
    mock.mockReset();
    mock.mockRejectedValue(new Error('boom'));

    const { refreshThreadEvents } = await import('./thread-loading');
    await refreshThreadEvents('R1');
    await runLoad('L1');

    expect(toasts.value.map(t => t.key).sort()).toEqual(
      [THREAD_EVENTS_LOAD_TOAST_KEY, THREAD_EVENTS_REFRESH_TOAST_KEY].sort(),
    );

    mock.mockReset();
    mock.mockResolvedValue({ events: [], currentAggregate: null });
    await refreshThreadEvents('R1');

    expect(toasts.value.map(t => t.key)).toEqual([THREAD_EVENTS_LOAD_TOAST_KEY]);
  });
});


describe('focusThread — section', () => {
  it('does not archive an inbox thread when focused', () => {
    const map = new Map<string, ThreadState>();
    map.set('t1', makeThreadState('t1', {
      meta: { id: 't1', title: 'Inbox Thread', channel: 'chat', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:00Z', status: 'idle', codingAgentChangeState: { kind: 'none' }, codingAgentIsExternalRepo: false, lastRevivedAt: '', messageCount: 0, section: 'inbox', activeChildrenCount: 0 },
    }));
    threadMap.value = map;

    focusThread('t1');

    expect(threadMap.value.get('t1')!.meta.section).toBe('inbox');
  });

  it('does not update threadMap when section is already archived', () => {
    const map = new Map<string, ThreadState>();
    map.set('t1', makeThreadState('t1'));
    threadMap.value = map;

    const mapBefore = threadMap.value;
    focusThread('t1');

    expect(threadMap.value).toBe(mapBefore);
  });
});


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
      archive: [
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
      meta: { id: 't1', title: '...', channel: 'chat', saved: false, createdAt: '', updatedAt: '2026-03-15T19:00:00Z', status: 'idle', codingAgentChangeState: { kind: 'none' }, codingAgentIsExternalRepo: false, lastRevivedAt: '', messageCount: 0, section: 'archived', activeChildrenCount: 0 },
    }));
    threadMap.value = map;

    (fetchThreads as any).mockResolvedValue({
      saved: [],
      active_threads: [],
      composing: [],
      active: [],
      archive: [
        { thread_id: 't1', title: 'Thread 1', channel: 'chat', last_activity: '2026-03-15T18:00:00Z', created_at: '2026-03-15T18:00:00Z', message_count: 1, section: 'inbox' },
      ],
    });

    await loadAllThreads();

    expect(threadMap.value.get('t1')!.meta.section).toBe('inbox');
  });
});


describe('upsertThread — updatedAt monotonic', () => {
  it('does not overwrite a newer SSE-derived updatedAt with a stale API last_activity', () => {
    const map = new Map<string, ThreadState>();
    map.set('t1', makeThreadState('t1', {
      meta: {
        id: 't1', title: 'CC Thread', channel: 'claude_code', saved: false,
        createdAt: '2026-03-31T19:21:38Z',
        updatedAt: '2026-03-31T19:31:54Z',
        status: 'running', codingAgentChangeState: { kind: 'none' },
        codingAgentIsExternalRepo: false, lastRevivedAt: '',
        messageCount: 0, section: 'archived', activeChildrenCount: 0,
      },
    }));

    upsertThread(map, {
      thread_id: 't1',
      title: 'CC Thread',
      channel: 'claude_code',
      last_activity: '2026-03-31T19:21:38Z',
      created_at: '2026-03-31T19:21:38Z',
      message_count: 1,
      section: 'archived',
      status: 'running',
      summaryVersion: 0,
    } as any, false);

    expect(map.get('t1')!.meta.updatedAt).toBe('2026-03-31T19:31:54Z');
  });

  it('advances updatedAt when API returns a newer last_activity', () => {
    const map = new Map<string, ThreadState>();
    map.set('t1', makeThreadState('t1', {
      meta: {
        id: 't1', title: 'Thread', channel: 'chat', saved: false,
        createdAt: '2026-03-31T10:00:00Z',
        updatedAt: '2026-03-31T10:00:00Z',
        status: 'idle', codingAgentChangeState: { kind: 'none' },
        codingAgentIsExternalRepo: false, lastRevivedAt: '',
        messageCount: 0, section: 'archived', activeChildrenCount: 0,
      },
    }));

    upsertThread(map, {
      thread_id: 't1',
      title: 'Thread',
      channel: 'chat',
      last_activity: '2026-03-31T12:00:00Z',
      created_at: '2026-03-31T10:00:00Z',
      message_count: 2,
      section: 'archived',
      status: 'idle',
      summaryVersion: 0,
    } as any, false);

    expect(map.get('t1')!.meta.updatedAt).toBe('2026-03-31T12:00:00Z');
  });
});
