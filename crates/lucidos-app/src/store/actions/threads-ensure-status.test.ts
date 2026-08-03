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
import { fetchThreads, fetchThreadById } from '../../api/threads';
import { drawerOpen } from '../../components/layout/Drawer';
import { _resetComposeDraftsForTesting } from '../composeDrafts';
import { archivingThreadIds, bootstrappingThreadId, focusedThreadId, generatedTitleIds, mobileView, resetCodingAgentPendingPreferences, threadDrawerOpen, threadMap, threadsLoaded, toasts } from '../store';
import { ensureThreadByIdInMap, ensureThreadInMap, loadAllThreads, upsertThread } from './thread-loading';
import { focusThread, focusThreadOrBootstrapResult } from './threads';

// Mock the API module
vi.mock('../../api/threads', () => ({
  fetchThreads: vi.fn(),
  fetchThreadById: vi.fn(),
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
  resetCodingAgentPendingPreferences();
  generatedTitleIds.clear();
  archivingThreadIds.value = new Set();
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
      total_children_count: 0,
      blocking_descendant_count: 0, attention_descendant_count: 0,
      status: 'idle',
      coding_agent_proposed: false,
      coding_agent_requires_restart: false,
      coding_agent_is_external_repo: false,
      coding_agent_applying: false, coding_agent_has_diff: false, last_revived_at: null,
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
      meta: { id: 't1', title: 'Existing Title', channel: 'claude_code', initiator: 'user', saved: true, createdAt: '', updatedAt: '2026-01-01T00:00:00Z', status: 'idle', codingAgentProposed: false, codingAgentRequiresRestart: false, codingAgentIsExternalRepo: false, codingAgentApplying: false, lastRevivedAt: '', messageCount: 5, section: 'archived', activeChildrenCount: 0 },
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
      blocking_descendant_count: 0, attention_descendant_count: 0,
      status: 'idle',
      coding_agent_proposed: false,
      coding_agent_requires_restart: false,
      coding_agent_is_external_repo: false,
      coding_agent_applying: false, coding_agent_has_diff: false, last_revived_at: null,
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
// past the per-source Archive window).
// ---------------------------------------------------------------------------

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
      coding_agent_proposed: false,
      coding_agent_requires_restart: false,
      coding_agent_is_external_repo: false,
      coding_agent_applying: false,
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
    // The grouped GET /api/v1/threads assembles saved + recent archive + active
    // + composing + the family base: p50 262ms of server time at ~5k threads,
    // paid to learn about one row. It sat on the notification-tap critical path
    // (a tap navigating to a thread outside the loaded window blocks here with
    // nothing on screen) and duplicated the grouped fetch the same cold boot had
    // already issued.
    threadMap.value = new Map();
    (fetchThreadById as any).mockResolvedValue(summary());

    await ensureThreadByIdInMap('old-archived-1');

    expect(fetchThreads).not.toHaveBeenCalled();
  });

  it('carries `saved` off the summary, so a saved thread does not land unsaved', async () => {
    // The grouped endpoint conveys saved structurally (membership in its `saved`
    // array); one bare summary cannot, so the flag rides on the row itself.
    // Guessing false here would show a saved thread as unsaved in the drawer.
    threadMap.value = new Map();
    (fetchThreadById as any).mockResolvedValue(summary({ saved: true }));

    await ensureThreadByIdInMap('old-archived-1');

    expect(threadMap.value.get('old-archived-1')!.meta.saved).toBe(true);
  });

  it('returns true without fetching when the thread is already in the map', async () => {
    const map = new Map<string, ThreadState>();
    map.set('already-here', makeThreadState('already-here', {
      meta: { id: 'already-here', title: 'Already Here', channel: 'chat', initiator: 'user', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:00Z', status: 'idle', codingAgentProposed: false, codingAgentRequiresRestart: false, codingAgentIsExternalRepo: false, codingAgentApplying: false, lastRevivedAt: '', messageCount: 1, section: 'inbox', activeChildrenCount: 0 },
    }));
    threadMap.value = map;

    const ok = await ensureThreadByIdInMap('already-here');
    expect(ok).toBe(true);
    expect(fetchThreadById).not.toHaveBeenCalled();
  });

  it('returns false when the API has no record of the thread (404)', async () => {
    threadMap.value = new Map();
    // fetchThreadById maps the engine's 404 to null: a real "gone" verdict.
    (fetchThreadById as any).mockResolvedValue(null);

    const ok = await ensureThreadByIdInMap('does-not-exist');
    expect(ok).toBe(false);
    expect(threadMap.value.has('does-not-exist')).toBe(false);
  });

  it('propagates fetch errors to the caller (no swallow)', async () => {
    // "Could not ask" must stay distinct from "gone": landThreadHash retries a
    // thrown failure (a peer engine lazy-starting behind the gateway routinely
    // fails the first request) and must never retry a 404.
    threadMap.value = new Map();
    (fetchThreadById as any).mockRejectedValue(new Error('network down'));

    await expect(ensureThreadByIdInMap('any-id')).rejects.toThrow('network down');
  });
});

// ---------------------------------------------------------------------------
// focusThreadOrBootstrapResult: the tap must be acknowledged before the
// metadata lands. 80% of this workspace's notifications navigate to a thread,
// and when the thread is outside the loaded window the old code awaited the
// fetch with nothing on screen: no pane movement, no skeleton, no cursor. On a
// cold push tap the map is always empty, so it happened on every single one.
// ---------------------------------------------------------------------------

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
      coding_agent_proposed: false,
      coding_agent_requires_restart: false,
      coding_agent_is_external_repo: false,
      coding_agent_applying: false,
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
    bootstrappingThreadId.value = null;
    (fetchThreadById as any).mockReset();
  });

  it('focuses the target and flags the bootstrap BEFORE the metadata arrives', async () => {
    const d = deferred<any>();
    (fetchThreadById as any).mockReturnValue(d.promise);

    const pending = focusThreadOrBootstrapResult('target');

    // Both true on this tick, with the fetch still in flight. The focus is what
    // moves the pane; the flag is what stops ThreadView's stale-pointer cleanup
    // from immediately undoing it (the thread is legitimately not in the map).
    expect(focusedThreadId.value).toBe('target');
    expect(bootstrappingThreadId.value).toBe('target');

    d.resolve(summary('target'));
    await pending;
  });

  it('clears the bootstrap flag once the thread is really in the map', async () => {
    (fetchThreadById as any).mockResolvedValue(summary('target'));

    const outcome = await focusThreadOrBootstrapResult('target');

    expect(outcome.kind).toBe('focused');
    expect(focusedThreadId.value).toBe('target');
    // Left set, it would exempt a genuinely stale pointer from cleanup later.
    expect(bootstrappingThreadId.value).toBeNull();
  });

  it('restores the previous focus when the thread does not exist', async () => {
    threadMap.value = new Map([['was-here', makeThreadState('was-here')]]);
    focusedThreadId.value = 'was-here';
    (fetchThreadById as any).mockResolvedValue(null);

    const outcome = await focusThreadOrBootstrapResult('ghost');

    expect(outcome.kind).toBe('not-found');
    // Not left staring at a skeleton for a thread that will never arrive.
    expect(focusedThreadId.value).toBe('was-here');
    expect(bootstrappingThreadId.value).toBeNull();
  });

  it('restores the previous focus when the fetch fails', async () => {
    threadMap.value = new Map([['was-here', makeThreadState('was-here')]]);
    focusedThreadId.value = 'was-here';
    (fetchThreadById as any).mockRejectedValue(new Error('network down'));

    const outcome = await focusThreadOrBootstrapResult('unreachable');

    expect(outcome.kind).toBe('failed');
    expect(focusedThreadId.value).toBe('was-here');
    expect(bootstrappingThreadId.value).toBeNull();
  });

  it('a superseded bootstrap does not yank focus off the newer one', async () => {
    // The user taps a second notification while the first is still in flight.
    const first = deferred<any>();
    const second = deferred<any>();
    (fetchThreadById as any)
      .mockReturnValueOnce(first.promise)
      .mockReturnValueOnce(second.promise);

    const p1 = focusThreadOrBootstrapResult('first');
    const p2 = focusThreadOrBootstrapResult('second');
    expect(bootstrappingThreadId.value).toBe('second');

    // The FIRST one now fails. It must not restore its own `previousFocus`,
    // which would drag the user off the thread they actually asked for last.
    first.reject(new Error('too late'));
    await p1;

    expect(focusedThreadId.value).toBe('second');
    expect(bootstrappingThreadId.value).toBe('second');

    second.resolve(summary('second'));
    await p2;
    expect(focusedThreadId.value).toBe('second');
    expect(bootstrappingThreadId.value).toBeNull();
  });

  it('a thread already in the map focuses synchronously and never flags a bootstrap', () => {
    threadMap.value = new Map([['warm', makeThreadState('warm')]]);

    void focusThreadOrBootstrapResult('warm');

    expect(focusedThreadId.value).toBe('warm');
    expect(bootstrappingThreadId.value).toBeNull();
    expect(fetchThreadById).not.toHaveBeenCalled();
  });
});

// ---------------------------------------------------------------------------
// Bug: CC thread spawned by chat shows in Archive instead of Active
// ---------------------------------------------------------------------------

describe('CC thread spawned by chat — status from API is authoritative', () => {
  it('API status always overwrites SSE skeleton status', async () => {
    // Scenario: SSE creates a CC thread skeleton with status='running'
    // (from CodingAgentThreadSpawned). Then loadAllThreads runs and the API doesn't
    // include this thread in the active set (Claude Code session hasn't registered yet).
    // Previously, upsertThread would set status='idle', causing the
    // thread to show in Archive instead of Active.
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
      codingAgentProposed: false,
      codingAgentRequiresRestart: false,
      codingAgentIsExternalRepo: false,
      codingAgentApplying: false,  // Set by SSE skeleton
        lastRevivedAt: '',
        messageCount: 0,
        section: 'archived',
        activeChildrenCount: 0,
      },
    }));
    threadMap.value = map;

    // API response includes the CC thread in archive but NOT in active set
    // (because the Claude Code session hasn't registered itself yet)
    (fetchThreads as any).mockResolvedValue({
      saved: [],
      active_threads: [],
      composing: [],
      active: [],  // CC thread NOT in active set
      archive: [
        { thread_id: 'cc-1', title: 'Fix OAuth URLs', channel: 'claude_code', last_activity: '2026-03-19T20:00:00Z', created_at: '2026-03-19T20:00:00Z', message_count: 1, section: 'archived', active_children_count: 0, status: 'idle', coding_agent_proposed: false, coding_agent_requires_restart: false, coding_agent_is_external_repo: false, coding_agent_applying: false, last_revived_at: null },
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
      codingAgentProposed: false,
      codingAgentRequiresRestart: false,
      codingAgentIsExternalRepo: false,
      codingAgentApplying: false,  // Was running before restart
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
      archive: [
        { thread_id: 'cc-1', title: 'Aborted Session', channel: 'claude_code', last_activity: '2026-03-19T20:00:00Z', created_at: '2026-03-19T20:00:00Z', message_count: 1, section: 'inbox', status: 'idle', coding_agent_proposed: false, coding_agent_requires_restart: false, coding_agent_is_external_repo: false, coding_agent_applying: false, last_revived_at: null, active_children_count: 0 },
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
          coding_agent_proposed: false,
          coding_agent_requires_restart: false,
          coding_agent_is_external_repo: false,
          coding_agent_applying: false, coding_agent_has_diff: false, last_revived_at: null,
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
      blockingDescendantCount: 0, attentionDescendantCount: 0,
      codingAgentProposed: false,
      codingAgentRequiresRestart: false,
      codingAgentIsExternalRepo: false,
      codingAgentApplying: false,
      isSaved: false,
      hasResponse: true,
      lastRevivedAt: null,
      parentThreadId: null,
      parentThreadTitle: null,
      state: 'active',
      latestTodoList: null,
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
      meta: { id: 'refresh-t1', title: 'Refresh Thread', channel: 'claude_code', saved: false, createdAt: '2026-03-28T19:00:00Z', updatedAt: '2026-03-28T19:00:00Z', status: 'idle', codingAgentProposed: false, codingAgentRequiresRestart: false, codingAgentIsExternalRepo: false, codingAgentApplying: false, lastRevivedAt: '', messageCount: 1, section: 'archived', activeChildrenCount: 0 },
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
      blockingDescendantCount: 0, attentionDescendantCount: 0,
      codingAgentProposed: false,
      codingAgentRequiresRestart: false,
      codingAgentIsExternalRepo: false,
      codingAgentApplying: false,
      isSaved: false,
      hasResponse: false,
      lastRevivedAt: '2026-03-30T10:00:00Z',
      parentThreadId: null,
      parentThreadTitle: null,
      state: 'active',
      latestTodoList: null,
    } });

    const { refreshThreadEvents } = await import('./thread-loading');
    await refreshThreadEvents('refresh-t1');

    // New event must update status — refreshThreadEvents does NOT preserve stale status
    expect(threadMap.value.get('refresh-t1')!.meta.status).toBe('running');
  });
});

// ---------------------------------------------------------------------------
// refreshThreadEvents — retry once on transient DOMException / transport error
// ---------------------------------------------------------------------------
// The runResumeSync / resyncLoadedThreads paths fire Promise.all of
// refreshThreadEvents for every loaded thread on SSE reconnect, iOS PWA wake,
// or right after an engine restart. On iOS Safari, the browser cancels
// in-flight fetches mid-flight (suspend/resume, lifecycle, network change) —
// surfacing as AbortError — and fails the first request on a stale HTTP/2
// connection — surfacing as TypeError "Load failed". A single retry covers the
// transient case; if both attempts still fail transiently we stay silent
// because SSE recovers. Without this the user gets one "Failed to refresh
// thread events" toast per affected thread, which can mean dozens of toasts in
// a single wake cycle. Mirrors the retry-on-TimeoutError pattern in
// refreshChangesState.

describe('refreshThreadEvents — retry on transient DOMException / transport error', () => {
  beforeEach(() => {
    toasts.value = [];
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
    // No toast — the refresh runs without user intent (resyncLoadedThreads
    // on wake / SSE reconnect) and SSE recovers any missed events. Toasting
    // surfaced a dozen errors per wake cycle on iOS PWA — visible noise
    // the user couldn't act on.
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
    // No toast — this is the exact pile the user reported after returning to a
    // backgrounded iOS PWA once the engine had restarted: the resync's fetches
    // hit a stale connection (TypeError "Load failed"). SSE recovers any missed
    // events; the refresh is background (runResumeSync), not user-initiated.
    expect(toasts.value.find(t => t.message?.startsWith('Failed to refresh thread events'))).toBeUndefined();
  });

  it('still shows the toast when both attempts fail with a non-Abort DOMException (e.g. TimeoutError)', async () => {
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
    const toast = toasts.value.find(t => t.message?.startsWith('Failed to refresh thread events'));
    expect(toast).toBeTruthy();
    expect(toast!.message).toBe('Failed to refresh thread events for "Refresh Retry Thread": request timed out');
    expect(toast!.type).toBe('error');
  });

  it('does not retry on a non-DOMException error (e.g. 500) — surfaces immediately', async () => {
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

// ---------------------------------------------------------------------------
// Section — focusThread does NOT auto-archive (user must click Archive/Apply/Discard)
// ---------------------------------------------------------------------------

describe('focusThread — section', () => {
  it('does not archive an inbox thread when focused', () => {
    const map = new Map<string, ThreadState>();
    map.set('t1', makeThreadState('t1', {
      meta: { id: 't1', title: 'Inbox Thread', channel: 'chat', saved: false, createdAt: '', updatedAt: '2026-01-01T00:00:00Z', status: 'idle', codingAgentProposed: false, codingAgentRequiresRestart: false, codingAgentIsExternalRepo: false, codingAgentApplying: false, lastRevivedAt: '', messageCount: 0, section: 'inbox', activeChildrenCount: 0 },
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
      meta: { id: 't1', title: '...', channel: 'chat', saved: false, createdAt: '', updatedAt: '2026-03-15T19:00:00Z', status: 'idle', codingAgentProposed: false, codingAgentRequiresRestart: false, codingAgentIsExternalRepo: false, codingAgentApplying: false, lastRevivedAt: '', messageCount: 0, section: 'archived', activeChildrenCount: 0 },
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

// ---------------------------------------------------------------------------
// Section — upsertThread must not overwrite updatedAt with stale API value
// ---------------------------------------------------------------------------

describe('upsertThread — updatedAt monotonic', () => {
  it('does not overwrite a newer SSE-derived updatedAt with a stale API last_activity', () => {
    // Scenario: Claude Code session is actively streaming. SSE events have updated
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
        status: 'running', codingAgentProposed: false,
        codingAgentRequiresRestart: false, codingAgentIsExternalRepo: false, codingAgentApplying: false, lastRevivedAt: '',
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
        status: 'idle', codingAgentProposed: false,
        codingAgentRequiresRestart: false, codingAgentIsExternalRepo: false, codingAgentApplying: false, lastRevivedAt: '',
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

