/**
 * @vitest-environment jsdom
 *
 * How the shell drives its event-stream transport, and what it must do
 * differently when that transport is the shared worker rather than its own
 * EventSource.
 *
 * The frame path is deliberately identical: a relayed frame and a direct one
 * are the same string, so everything downstream is untouched. What differs is
 * who retries, and dropping our port on a shared connection would take the
 * stream down for every other document of the workspace.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { signal } from '@preact/signals-core';
import type { StreamDrop, StreamHealth } from '@lucidos/event-stream';
import { gapClearedProgress, gapRefreshedVersion } from '../eventGap';

const postClientLog = vi.fn();
vi.mock('../../utils/clientLog', () => ({ postClientLog }));

const ERROR: StreamDrop = { reason: 'error' };

const refreshThreadEvents = vi.fn(async (_id: string) => {});
const markLoadedThreadsStale = vi.fn();
vi.mock('./thread-loading', () => ({ refreshThreadEvents, markLoadedThreadsStale }));

const refreshThreadList = vi.fn(async () => {});
vi.mock('./thread-list-refresh', () => ({ refreshThreadList }));

/** The transport the shell opened, captured so the test can drive its
 *  handlers. Standing in for a browser the unit test does not have. */
let opened: {
  handlers: { onFrame: (d: string) => void; onOpen: () => void; onError: (drop: StreamDrop) => void };
  opts: { pongs: boolean };
  stream: {
    close: ReturnType<typeof vi.fn>;
    ownsReconnect: boolean;
    submitPong: ReturnType<typeof vi.fn>;
    revalidate: ReturnType<typeof vi.fn>;
  };
} | null = null;

/** Whether the next transport claims to retry for itself. */
let nextOwnsReconnect = false;

/** What the next transport answers when asked to revalidate. */
let nextHealth: StreamHealth = 'live';

/** The targets the shell asked for, so the URL construction is covered too. */
let openedTargets: { streamUrl: string; pongUrl: string; workerUrl: string } | null = null;

const openEventStream = vi.fn((targets, handlers, opts) => {
  const stream = {
    close: vi.fn(),
    ownsReconnect: nextOwnsReconnect,
    submitPong: vi.fn(),
    revalidate: vi.fn(() => nextHealth),
  };
  openedTargets = targets;
  opened = { handlers, opts, stream };
  return stream;
});
// Only the transport opener is stubbed. `eventStreamTargets` stays real, so a
// renamed route would fail the URL assertion below rather than pass a stub.
vi.mock('@lucidos/event-stream', async (importOriginal) => ({
  ...(await importOriginal<Record<string, unknown>>()),
  openEventStream,
}));

/** What app frames hear about the stream, through the host relay. */
const fanOutEventStreamStatus = vi.fn();
vi.mock('./app-bridge', async (importOriginal) => ({
  ...(await importOriginal<Record<string, unknown>>()),
  fanOutEventStreamStatus,
}));

const threadMap = signal(new Map<string, { meta: { id: string }; eventsLoaded: boolean }>());
const focusedThreadId = signal<string | null>(null);
const recommendedCleanupProgress = gapClearedProgress<{ done: number; total: number }>();
const diskUsageVersion = gapRefreshedVersion();
vi.mock('../store', () => ({
  threadMap,
  focusedThreadId,
  promptFootprintVersion: signal(0),
  changes: signal([]),
  appliedChanges: signal([]),
  changesHasMore: signal(false),
  updateAvailable: signal(false),
  applyingChangeIds: signal(new Set()),
  applyingNowThreadIds: signal(new Map()),
  generatedTitleIds: new Set(),
  codingAgentSessionVersion: signal(0),
  memoryRebuildProgress: signal(null),
  backupProgress: signal(null),
  recommendedCleanupProgress,
  diskUsageVersion,
  recoveryProgress: signal(null),
  panelOverlay: signal(null),
  showConfirm: vi.fn(),
  showToast: vi.fn(),
  dismissToast: vi.fn(),
  repoSource: signal(null),
}));

vi.mock('../../api/client', () => ({ API_BASE: '', API: '/api/v1', postMcpConsent: vi.fn() }));
vi.mock('../thread-events', () => ({
  handleEvent: vi.fn(),
  isChannelDefiningEvent: vi.fn(() => false),
  makeOptimisticThreadState: vi.fn(),
  modeToInitiator: vi.fn(),
  PENDING_TITLE_PLACEHOLDER: '',
}));
const loadUnreadNotifications = vi.fn(async () => {});
vi.mock('./notifications', () => ({ handleNotificationSSE: vi.fn(), loadUnreadNotifications }));
const refreshChangesState = vi.fn(async () => {});
vi.mock('./chat-changes', () => ({ addRestartGroup: vi.fn(), refreshChangesState }));
const loadThreadQueue = vi.fn(async () => {});
vi.mock('./threadQueue', () => ({ loadThreadQueue }));
const loadPreferences = vi.fn(async () => {});
vi.mock('./preferences', () => ({ loadPreferences }));
const refreshArtifacts = vi.fn();
vi.mock('./artifacts', () => ({
  loadArtifacts: vi.fn(),
  refreshArtifacts,
  openFilePreview: vi.fn(),
  openUrl: vi.fn(),
  normalizeDataPath: vi.fn(),
}));
vi.mock('./triggers', () => ({ navigateToTrigger: vi.fn() }));
vi.mock('./apps', () => ({ refreshAppUI: vi.fn(), captureAppUI: vi.fn(), openAppById: vi.fn() }));
vi.mock('./wipPreview', () => ({ clearWipIfMatches: vi.fn() }));
vi.mock('./credentials', () => ({ openCredentialRequest: vi.fn() }));
vi.mock('./menu', () => ({
  setActiveMenu: vi.fn(),
  switchMenuItem: vi.fn(),
  openSettingsSubview: vi.fn(),
  openBackupSettings: vi.fn(),
}));
vi.mock('./navigation', () => ({ pushNavState: vi.fn(), replaceNavState: vi.fn() }));
vi.mock('./push', () => ({ setDevicePushEnabled: vi.fn() }));
vi.mock('./devices', () => ({ getDeviceId: vi.fn() }));
vi.mock('./threads', () => ({ focusThread: vi.fn() }));
vi.mock('./repositories', () => ({ refreshRepoView: vi.fn(), openEncodedRepoFilePreview: vi.fn(() => false) }));

const syncPendingFormRequests = vi.fn(async () => {});
vi.mock('./form-requests', () => ({
  syncPendingFormRequests,
  openFormRequest: vi.fn(),
  closeResolvedFormRequest: vi.fn(),
}));

const syncClientUpdateFromBuild = vi.fn(async () => {});
vi.mock('./client-update', () => ({ syncClientUpdateFromBuild }));

const processSSEForReferences = vi.fn();
vi.mock('./entityReferences', () => ({
  processSSEForReferences,
  refreshLlmConfigured: vi.fn(),
  PROVIDER_PREFERENCE_KEYS: new Set(['opencode_free_enabled', 'provider_enabled_openai']),
}));

const { connectThreadEvents, disconnectThreadEvents, reviveThreadEvents } = await import('./thread-sync');
const { NO_BACKFILL_PROGRESS, applyTreeBackfillProgress, treeBackfill } = await import('./treeBackfill');

/** What the shell reports to the DOM, which is the only thing telling a user
 *  the stream is down. */
const status = () => document.documentElement.dataset.lucidosEventStream;

describe('the shell attaching to a transport', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.useFakeTimers();
    opened = null;
    nextOwnsReconnect = false;
    nextHealth = 'live';
    threadMap.value = new Map();
    focusedThreadId.value = null;
  });

  afterEach(() => {
    disconnectThreadEvents();
    vi.useRealTimers();
  });

  it('registers as a ponger, because the shell is what answers a PresenceCheck', () => {
    connectThreadEvents();
    expect(opened?.opts).toEqual({ pongs: true });
  });

  it('builds its three URLs off the one versioned API base', () => {
    // The shell and an app reach that base differently. They must still name
    // the same routes, so the suffixes come from the SDK, not from either side.
    connectThreadEvents();
    expect(openedTargets).toEqual({
      streamUrl: '/api/v1/events',
      pongUrl: '/api/v1/presence-pong',
      workerUrl: '/api/v1/sse-worker.js',
    });
  });

  it('routes a frame into the store whichever transport delivered it', () => {
    // The equivalence the whole design rests on. `onFrame` takes the same
    // string a direct EventSource would have handed over, so a relayed frame
    // is indistinguishable from here down.
    connectThreadEvents();
    opened?.handlers.onFrame('{"type":"NotificationCreated","data":{"id":"n-1"}}');

    expect(processSSEForReferences).toHaveBeenCalledWith('NotificationCreated', { id: 'n-1' });
  });

  it('marks connected on open and disconnected on error', () => {
    // A follower must never read as connected while nothing is arriving.
    connectThreadEvents();
    expect(status()).toBe('connecting');

    opened?.handlers.onOpen();
    expect(status()).toBe('connected');

    opened?.handlers.onError(ERROR);
    expect(status()).toBe('disconnected');
  });

  it('tells app frames about a teardown it makes itself, then the reopen', () => {
    // The resume path disconnects and reconnects on purpose. By then the
    // closed transport's own error handler is stale. Without this, a frame
    // keeps believing the stream is up and misses the reopen it resyncs on.
    connectThreadEvents();
    opened?.handlers.onOpen();
    expect(fanOutEventStreamStatus).toHaveBeenLastCalledWith('open');

    disconnectThreadEvents();
    expect(fanOutEventStreamStatus).toHaveBeenLastCalledWith('error');

    connectThreadEvents();
    opened?.handlers.onOpen();
    expect(fanOutEventStreamStatus).toHaveBeenLastCalledWith('open');
  });

  describe('reviving the stream on a resume', () => {
    it('opens a stream when there is none', () => {
      reviveThreadEvents();
      expect(openEventStream).toHaveBeenCalledTimes(1);
    });

    it.each<StreamHealth>(['live', 'checking'])('keeps a %s stream, so a frame emitted meanwhile still arrives', (health) => {
      connectThreadEvents();
      opened?.handlers.onOpen();
      const first = opened!;
      nextHealth = health;
      fanOutEventStreamStatus.mockClear();

      reviveThreadEvents();

      expect(first.stream.close).not.toHaveBeenCalled();
      expect(openEventStream).toHaveBeenCalledTimes(1);
      expect(fanOutEventStreamStatus).not.toHaveBeenCalled();
      first.handlers.onFrame('{"type":"NotificationCreated","data":{"id":"n-2"}}');
      expect(processSSEForReferences).toHaveBeenCalledWith('NotificationCreated', { id: 'n-2' });
    });

    it('replaces a stale stream with a fresh one', () => {
      connectThreadEvents();
      opened?.handlers.onOpen();
      const first = opened!;
      first.stream.revalidate.mockReturnValue('stale');

      reviveThreadEvents();

      expect(first.stream.close).toHaveBeenCalledTimes(1);
      expect(openEventStream).toHaveBeenCalledTimes(2);
      first.handlers.onFrame('{"type":"NotificationCreated","data":{"id":"n-3"}}');
      expect(processSSEForReferences).not.toHaveBeenCalledWith('NotificationCreated', { id: 'n-3' });
    });
  });

  it('reconciles on the open that follows an error, not on the first one', async () => {
    // The first open is a page load, where startClient has already read state.
    // Every later one follows a gap whose frames nobody replayed.
    connectThreadEvents();
    opened?.handlers.onOpen();
    expect(refreshThreadList).not.toHaveBeenCalled();

    opened?.handlers.onError(ERROR);
    opened?.handlers.onOpen();
    await vi.runAllTimersAsync();

    expect(refreshThreadList).toHaveBeenCalledTimes(1);
  });

  describe('the backfill frame order', () => {
    const at = (seq: number, done: number) => ({ ...NO_BACKFILL_PROGRESS, total: 2, done, seq });
    const shows = (progress: ReturnType<typeof at>) =>
      expect(treeBackfill.value).toEqual({ status: 'loaded', data: { state: 'running', progress } });

    it('takes a restarted engine\'s frames after a resume sync', () => {
      connectThreadEvents();
      opened?.handlers.onOpen();
      applyTreeBackfillProgress(at(40, 0));

      disconnectThreadEvents();
      connectThreadEvents();
      opened?.handlers.onOpen();
      applyTreeBackfillProgress(at(1, 1));
      shows(at(1, 1));
    });

    it('takes a restarted engine\'s frames after a dropped stream', () => {
      connectThreadEvents();
      opened?.handlers.onOpen();
      applyTreeBackfillProgress(at(40, 0));

      opened?.handlers.onError(ERROR);
      vi.runOnlyPendingTimers();
      opened?.handlers.onOpen();
      applyTreeBackfillProgress(at(1, 1));
      shows(at(1, 1));
    });

    /** A snapshot can land before the first open, and a frame it already
     *  counted can still arrive after. */
    it('keeps the order a snapshot set before the first open', () => {
      applyTreeBackfillProgress(at(5, 1));
      connectThreadEvents();
      opened?.handlers.onOpen();
      applyTreeBackfillProgress(at(4, 0));
      shows(at(5, 1));
    });
  });

  it('re-checks the served client build on the open that follows an error', () => {
    // A served-snapshot swap during the gap sent a transient
    // ServedFrontendAdvanced that nobody replays. The first open is covered by
    // startClient's own check.
    connectThreadEvents();
    opened?.handlers.onOpen();
    expect(syncClientUpdateFromBuild).not.toHaveBeenCalled();

    opened?.handlers.onError(ERROR);
    opened?.handlers.onOpen();
    expect(syncClientUpdateFromBuild).toHaveBeenCalledTimes(1);
  });

  it('drops a stale cleanup cue and has Disk Usage re-read on the open that follows an error', () => {
    // A RecommendedCleanupCompleted sent during the gap is never replayed, so
    // the cue would read "Freeing…" for good. The page's summary re-sets it.
    connectThreadEvents();
    opened?.handlers.onOpen();
    recommendedCleanupProgress.value = { done: 4, total: 10 };
    const before = diskUsageVersion.value;

    opened?.handlers.onError(ERROR);
    opened?.handlers.onOpen();

    expect(recommendedCleanupProgress.value).toBeNull();
    expect(diskUsageVersion.value).toBe(before + 1);
  });

  it('refreshes the Files list on EVERY open, the first one included', () => {
    // A file written before an open was announced to nobody. The first open
    // counts: the page's first listing can finish before it.
    connectThreadEvents();
    expect(refreshArtifacts).not.toHaveBeenCalled();
    opened?.handlers.onOpen();
    expect(refreshArtifacts).toHaveBeenCalledTimes(1);

    opened?.handlers.onError(ERROR);
    opened?.handlers.onOpen();
    expect(refreshArtifacts).toHaveBeenCalledTimes(2);
  });

  it('reads the unread notifications on EVERY open, the first one included', () => {
    // A notification created before an open was announced to nobody. The
    // first open counts: the page's startup read can finish before it.
    connectThreadEvents();
    expect(loadUnreadNotifications).not.toHaveBeenCalled();
    opened?.handlers.onOpen();
    expect(loadUnreadNotifications).toHaveBeenCalledTimes(1);

    opened?.handlers.onError(ERROR);
    opened?.handlers.onOpen();
    expect(loadUnreadNotifications).toHaveBeenCalledTimes(2);
  });

  it('reads the preferences on EVERY open, the first one included', () => {
    // A preference written before an open reaches the page only here. The
    // first open counts: the startup read can finish before it.
    connectThreadEvents();
    expect(loadPreferences).not.toHaveBeenCalled();
    opened?.handlers.onOpen();
    expect(loadPreferences).toHaveBeenCalledTimes(1);

    opened?.handlers.onError(ERROR);
    opened?.handlers.onOpen();
    expect(loadPreferences).toHaveBeenCalledTimes(2);
  });

  it('reads the open form requests on EVERY open, the first one included', () => {
    // A form request emitted while no stream was up reaches the page only
    // here. The first open counts: a reload drops every frame before it.
    connectThreadEvents();
    expect(syncPendingFormRequests).not.toHaveBeenCalled();
    opened?.handlers.onOpen();
    expect(syncPendingFormRequests).toHaveBeenCalledTimes(1);

    opened?.handlers.onError(ERROR);
    opened?.handlers.onOpen();
    expect(syncPendingFormRequests).toHaveBeenCalledTimes(2);
  });

  it('reads the open form requests when the broadcast lags', () => {
    connectThreadEvents();
    opened?.handlers.onFrame('{"type":"Lagged","data":{"count":12}}');
    expect(syncPendingFormRequests).toHaveBeenCalledTimes(1);
  });

  it('reads the changes and the queue on the open that follows an error, not on the first one', () => {
    // An ApplyAllBatchCompleted sent during the gap is never replayed, so the
    // Apply All row would read "in progress" until a wake or a reload.
    connectThreadEvents();
    opened?.handlers.onOpen();
    expect(refreshChangesState).not.toHaveBeenCalled();
    expect(loadThreadQueue).not.toHaveBeenCalled();

    opened?.handlers.onError(ERROR);
    opened?.handlers.onOpen();
    expect(refreshChangesState).toHaveBeenCalledTimes(1);
    expect(loadThreadQueue).toHaveBeenCalledTimes(1);
  });

  it('reads the changes and the queue when the broadcast lags', () => {
    // A lag drops frames with the stream still open, so no reconnect follows.
    connectThreadEvents();
    opened?.handlers.onOpen();
    opened?.handlers.onFrame('{"type":"Lagged","data":{"count":12}}');
    expect(refreshChangesState).toHaveBeenCalledTimes(1);
    expect(loadThreadQueue).toHaveBeenCalledTimes(1);
  });
});

describe('the one repair after an event gap', () => {
  // Registered by the test, so these hold for any signal a future change adds.
  const version = gapRefreshedVersion();
  const progress = gapClearedProgress<{ done: number }>();

  beforeEach(() => {
    vi.clearAllMocks();
    vi.useFakeTimers();
    opened = null;
    threadMap.value = new Map();
    focusedThreadId.value = null;
    progress.value = { done: 1 };
  });

  afterEach(() => {
    disconnectThreadEvents();
    vi.useRealTimers();
  });

  it('repairs on the open that follows an error, once', () => {
    connectThreadEvents();
    opened?.handlers.onOpen();
    const before = version.value;
    expect(progress.value).not.toBeNull();

    opened?.handlers.onError(ERROR);
    opened?.handlers.onOpen();

    expect(version.value).toBe(before + 1);
    expect(progress.value).toBeNull();
  });

  it('leaves the first open alone', () => {
    const before = version.value;
    connectThreadEvents();
    opened?.handlers.onOpen();

    expect(version.value).toBe(before);
    expect(progress.value).not.toBeNull();
  });

  it('repairs when the broadcast lags, with the stream still open', () => {
    connectThreadEvents();
    opened?.handlers.onOpen();
    const before = version.value;

    opened?.handlers.onFrame('{"type":"Lagged","data":{"count":12}}');

    expect(version.value).toBe(before + 1);
    expect(progress.value).toBeNull();
  });

  it('serves a lag the way it serves a reconnect', async () => {
    // One list of repairs, so neither path can lack what the other has.
    const repairs = [markLoadedThreadsStale, refreshChangesState, loadThreadQueue, syncPendingFormRequests, syncClientUpdateFromBuild, refreshArtifacts, loadUnreadNotifications, loadPreferences];
    const callCounts = () => repairs.map((fn) => fn.mock.calls.length);
    connectThreadEvents();
    opened?.handlers.onOpen();
    await vi.runAllTimersAsync();

    vi.clearAllMocks();
    opened?.handlers.onFrame('{"type":"Lagged","data":{"count":12}}');
    await vi.runAllTimersAsync();
    const afterLag = callCounts();

    opened?.handlers.onError(ERROR);
    vi.clearAllMocks();
    opened?.handlers.onOpen();
    await vi.runAllTimersAsync();

    expect(afterLag).toEqual(callCounts());
  });
});

describe('who retries after a drop', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.useFakeTimers();
    opened = null;
    threadMap.value = new Map();
    focusedThreadId.value = null;
  });

  afterEach(() => {
    disconnectThreadEvents();
    vi.useRealTimers();
  });

  it('tears its own direct stream down and rebuilds it', () => {
    // WebKit's native retry strands a resumed iOS PWA, so the shell has always
    // done this itself for a connection it owns.
    nextOwnsReconnect = false;
    connectThreadEvents();
    const first = opened!.stream;

    opened?.handlers.onError(ERROR);
    expect(first.close).toHaveBeenCalledOnce();

    vi.advanceTimersByTime(3000);
    expect(openEventStream).toHaveBeenCalledTimes(2);
  });

  it('leaves a shared stream alone, because the worker owns the retry', () => {
    // Dropping our port here would leave the worker, and the last port leaving
    // takes the upstream down for every other document of the workspace.
    nextOwnsReconnect = true;
    connectThreadEvents();
    const only = opened!.stream;

    opened?.handlers.onError(ERROR);

    expect(only.close).not.toHaveBeenCalled();
    vi.advanceTimersByTime(10_000);
    expect(openEventStream).toHaveBeenCalledTimes(1);
  });

  it('still reports disconnected and still reconciles on the worker next open', async () => {
    // Not retrying is not the same as not noticing. The status has to move and
    // the resync has to be armed, or a follower goes quietly stale.
    nextOwnsReconnect = true;
    connectThreadEvents();
    opened?.handlers.onOpen();

    opened?.handlers.onError(ERROR);
    expect(status()).toBe('disconnected');

    opened?.handlers.onOpen();
    await vi.runAllTimersAsync();

    expect(status()).toBe('connected');
    expect(refreshThreadList).toHaveBeenCalledTimes(1);
  });
});

describe('a stalled stream', () => {
  const stalled: StreamDrop = { reason: 'stalled', silentMs: 35_000, transport: 'direct' };

  beforeEach(() => {
    vi.clearAllMocks();
    vi.useFakeTimers();
    opened = null;
    nextOwnsReconnect = false;
    threadMap.value = new Map();
    focusedThreadId.value = null;
  });

  afterEach(() => {
    disconnectThreadEvents();
    vi.useRealTimers();
  });

  it('posts one log line with the silence length and the transport', () => {
    connectThreadEvents();
    opened?.handlers.onOpen();

    opened?.handlers.onError(stalled);

    expect(postClientLog).toHaveBeenCalledOnce();
    expect(postClientLog).toHaveBeenCalledWith('event-stream', 'stalled', { silentMs: 35_000, transport: 'direct' });
  });

  it('posts nothing for a plain error', () => {
    connectThreadEvents();
    opened?.handlers.onOpen();

    opened?.handlers.onError(ERROR);

    expect(postClientLog).not.toHaveBeenCalled();
  });

  it('posts nothing from a handler a newer connection replaced', () => {
    connectThreadEvents();
    const stale = opened!.handlers;
    disconnectThreadEvents();
    connectThreadEvents();

    stale.onError(stalled);

    expect(postClientLog).not.toHaveBeenCalled();
  });

  it('arms the resync, so the next open re-reads loaded threads', async () => {
    connectThreadEvents();
    opened?.handlers.onOpen();
    await vi.runAllTimersAsync();
    expect(refreshThreadList).not.toHaveBeenCalled();

    opened?.handlers.onError(stalled);
    expect(status()).toBe('disconnected');
    opened?.handlers.onOpen();
    await vi.runAllTimersAsync();

    expect(refreshThreadList).toHaveBeenCalledTimes(1);
  });
});
