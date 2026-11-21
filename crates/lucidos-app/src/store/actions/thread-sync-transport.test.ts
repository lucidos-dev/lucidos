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
import { engineVersionStatusVersion } from '../engineVersionStatus';
import type { RestoreAdmissionPhase } from '../../utils/restoreAdmission';

const postClientLog = vi.fn();
vi.mock('../../utils/clientLog', () => ({ postClientLog }));

const ERROR: StreamDrop = { reason: 'error' };
let restoreRuntimeActive = false;
let restoreEpoch = 0;
const acceptRestoreHello = vi.fn(async (_epoch: number, _generation: string | null) => true);
vi.mock('../restoreRuntime', () => ({
  hasRestoreRuntime: () => restoreRuntimeActive,
  beginRestoreConnection: () => ++restoreEpoch,
  acceptRestoreHello,
}));
afterEach(() => { restoreRuntimeActive = false; });

let admissionPhase: RestoreAdmissionPhase = 'admitted';
vi.mock('../../utils/restoreAdmission', async (importOriginal) => ({
  ...(await importOriginal<Record<string, unknown>>()),
  restoreAdmissionPhase: () => admissionPhase,
}));
afterEach(() => { admissionPhase = 'admitted'; });

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
    restart: ReturnType<typeof vi.fn>;
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
    restart: vi.fn(),
  };
  openedTargets = targets;
  opened = { handlers, opts, stream };
  return stream;
});
const openDirectEventStream = vi.fn((targets, handlers) => openEventStream(targets, handlers, {pongs: true}));
let remoteGateway: string | null = null;
vi.mock('../../utils/basePath', async (importOriginal) => ({
  ...(await importOriginal<Record<string, unknown>>()),
  get REMOTE_GATEWAY() { return remoteGateway; },
}));
afterEach(() => { remoteGateway = null; });
// Only the transport opener is stubbed. `eventStreamTargets` stays real, so a
// renamed route would fail the URL assertion below rather than pass a stub.
vi.mock('@lucidos/event-stream', async (importOriginal) => ({
  ...(await importOriginal<Record<string, unknown>>()),
  openEventStream,
  openDirectEventStream,
}));

/** What app frames hear about the stream, through the host relay. */
const fanOutEventStreamStatus = vi.fn();
const fanOutEventFrame = vi.fn();
vi.mock('./app-frame-channel', async (importOriginal) => ({
  ...(await importOriginal<Record<string, unknown>>()),
  fanOutEventStreamStatus,
  fanOutEventFrame,
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
  removeToast: vi.fn(),
  repoSource: signal(null),
}));

vi.mock('../../api/client', async () => ({
  API_BASE: '',
  API: '/api/v1',
  postMcpConsent: vi.fn(),
  isTransientFetchError: (await vi.importActual<typeof import('../../api/client/_core')>('../../api/client/_core')).isTransientFetchError,
}));
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

// Existing cases exercise the warm dispatcher; cold-start cases below hold it.
let sinkLoaded = true;
const sinkPreload = vi.fn(async () => true);
const idleSinks: Array<{ preload(): Promise<unknown> }> = [];
vi.mock('../../utils/idlePrefetch', () => ({
  prefetchWhenIdle: (surface: { preload(): Promise<unknown> }) => idleSinks.push(surface),
}));
let sink: typeof import('./thread-sync');
vi.mock('../../utils/lazyComponent', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../../utils/lazyComponent')>();
  return { ...actual, lazyModule: (loader: () => Promise<unknown>) =>
    loader.toString().includes('thread-sync')
      ? { isLoaded: () => sinkLoaded, preload: sinkPreload, get: () => sink }
      : actual.lazyModule(loader) };
});
sink = await import('./thread-sync');
const { connectThreadEvents, disconnectThreadEvents, reviveThreadEvents } = await import('./thread-stream');
const { NO_BACKFILL_PROGRESS, applyTreeBackfillProgress, treeBackfill } = await import('./treeBackfill');
const { RESTORE_ADMISSION_STALL_MS, noteRestoreAdmitted } = await import('../restoreAdmissionWatchdog');
const { DeviceDatabaseReleasedError, DeviceDatabaseStalledError } = await import('../deviceDatabase');
const { showToast, removeToast } = await import('../store');
const { noteProvisionalRead } = await import('../../utils/restoreAdmission');

/** What the shell reports to the DOM, which is the only thing telling a user
 *  the stream is down. */
const status = () => document.documentElement.dataset.lucidosEventStream;

describe('production restore generation admission', () => {
  afterEach(() => { disconnectThreadEvents(); });

  it('queues domain frames behind durable hello admission and announces open afterward', async () => {
    vi.useRealTimers();
    vi.clearAllMocks();
    disconnectThreadEvents();
    restoreRuntimeActive = true;
    let admit!: (accepted: boolean) => void;
    acceptRestoreHello.mockImplementationOnce(() => new Promise<boolean>((resolve) => { admit = resolve; }));
    connectThreadEvents();
    opened!.handlers.onOpen();
    const hello = JSON.stringify({ type: 'StreamHello', data: { restore_generation: null } });
    opened!.handlers.onFrame(hello);
    opened!.handlers.onFrame(JSON.stringify({ type: 'RestoreAdmissionTestFrame', data: { value: 1 } }));
    await vi.waitFor(() => expect(acceptRestoreHello).toHaveBeenCalled());
    expect(fanOutEventStreamStatus).not.toHaveBeenCalledWith('open');
    expect(processSSEForReferences).not.toHaveBeenCalled();
    admit(true);
    await vi.waitFor(() => expect(processSSEForReferences).toHaveBeenCalledWith('RestoreAdmissionTestFrame', { value: 1 }, undefined));
    expect(fanOutEventStreamStatus).toHaveBeenCalledWith('open');
  });

  // A connection sends one hello. A failed admission must open a new one, or
  // every request parked behind admission would wait forever.
  it('reconnects to retry admission after a hello fails', async () => {
    vi.useFakeTimers();
    vi.clearAllMocks();
    disconnectThreadEvents();
    restoreRuntimeActive = true;
    acceptRestoreHello.mockRejectedValueOnce(new Error('storage full'));
    connectThreadEvents();
    const first = opened!;
    first.handlers.onOpen();
    first.handlers.onFrame(JSON.stringify({ type: 'StreamHello', data: { restore_generation: null } }));
    await vi.waitFor(() => expect(first.stream.close).toHaveBeenCalled());
    expect(openEventStream).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(10_000);
    expect(openEventStream).toHaveBeenCalledTimes(2);
    vi.useRealTimers();
  });

  // The failed connection dropped every frame it held back, so the retry must
  // reconcile the gap rather than run the first-open read.
  it('reconciles the gap on the connection that retries a failed hello', async () => {
    vi.useFakeTimers();
    vi.clearAllMocks();
    disconnectThreadEvents();
    restoreRuntimeActive = true;
    acceptRestoreHello.mockRejectedValueOnce(new Error('storage full'));
    connectThreadEvents();
    const first = opened!;
    first.handlers.onOpen();
    first.handlers.onFrame(JSON.stringify({ type: 'StreamHello', data: { restore_generation: null } }));
    await vi.waitFor(() => expect(first.stream.close).toHaveBeenCalled());
    await vi.advanceTimersByTimeAsync(10_000);
    const retry = opened!;
    retry.handlers.onOpen();
    retry.handlers.onFrame(JSON.stringify({ type: 'StreamHello', data: { restore_generation: null } }));
    await vi.waitFor(() => expect(fanOutEventStreamStatus).toHaveBeenCalledWith('open'));
    await vi.runAllTimersAsync();
    expect(refreshThreadList).toHaveBeenCalledTimes(1);
    vi.useRealTimers();
  });

  // Frames are dropped until admission, so a thread read that ran before it
  // left a gap the first open's reads do not cover.
  it.each([
    ['reconciles the gap after a provisional read', true, 1],
    ['adds nothing to the first admission without one', false, 0],
  ])('%s', async (_name, provisionalRead, reconciles) => {
    vi.useFakeTimers();
    vi.clearAllMocks();
    disconnectThreadEvents();
    restoreRuntimeActive = true;
    connectThreadEvents();
    if (provisionalRead) noteProvisionalRead();
    opened!.handlers.onOpen();
    opened!.handlers.onFrame(JSON.stringify({ type: 'StreamHello', data: { restore_generation: null } }));
    await vi.waitFor(() => expect(fanOutEventStreamStatus).toHaveBeenCalledWith('open'));
    await vi.runAllTimersAsync();
    expect(refreshThreadList).toHaveBeenCalledTimes(reconciles);
    vi.useRealTimers();
  });

  it('discards frames when a restore hello requests reload instead of admission', async () => {
    vi.useRealTimers();
    vi.clearAllMocks();
    disconnectThreadEvents();
    restoreRuntimeActive = true;
    acceptRestoreHello.mockResolvedValueOnce(false);
    connectThreadEvents();
    opened!.handlers.onOpen();
    opened!.handlers.onFrame(JSON.stringify({ type: 'StreamHello', data: { restore_generation: null } }));
    opened!.handlers.onFrame(JSON.stringify({ type: 'RestoreAdmissionTestFrame', data: {} }));
    await vi.waitFor(() => expect(acceptRestoreHello).toHaveBeenCalled());
    await Promise.resolve(); await Promise.resolve();
    expect(fanOutEventStreamStatus).not.toHaveBeenCalledWith('open');
    expect(processSSEForReferences).not.toHaveBeenCalled();
  });
});

describe('a stalled restore admission', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    disconnectThreadEvents();
    // Earlier suites leave failed admissions behind, whose wait still counts.
    noteRestoreAdmitted();
    vi.clearAllMocks();
    restoreRuntimeActive = true;
  });
  afterEach(() => {
    disconnectThreadEvents();
    vi.useRealTimers();
  });

  const restoreLogs = () => postClientLog.mock.calls.filter(([category]) => category === 'restore-admission');

  it('restarts a stream whose hello has not come, once per connection epoch', async () => {
    admissionPhase = 'awaiting-hello';
    nextOwnsReconnect = true;
    connectThreadEvents();
    const shared = opened!;
    shared.handlers.onOpen();
    await vi.advanceTimersByTimeAsync(RESTORE_ADMISSION_STALL_MS * 3);
    expect(shared.stream.restart).toHaveBeenCalledTimes(1);
    expect(restoreLogs()).toEqual([['restore-admission', 'stalled',
      { phase: 'awaiting-hello', waited_ms: RESTORE_ADMISSION_STALL_MS }]]);
    // The restart drops and reopens the upstream: a new epoch, one more try.
    shared.handlers.onError(ERROR);
    shared.handlers.onOpen();
    await vi.advanceTimersByTimeAsync(RESTORE_ADMISSION_STALL_MS);
    expect(shared.stream.restart).toHaveBeenCalledTimes(2);
    nextOwnsReconnect = false;
  });

  it('reports the total wait when the hello finally admits', async () => {
    admissionPhase = 'awaiting-hello';
    connectThreadEvents();
    opened!.handlers.onOpen();
    await vi.advanceTimersByTimeAsync(RESTORE_ADMISSION_STALL_MS);
    admissionPhase = 'admitted';
    opened!.handlers.onFrame(JSON.stringify({ type: 'StreamHello', data: { restore_generation: null } }));
    // Flushes the frame chain without moving the clock, as `waitFor` would.
    await vi.advanceTimersByTimeAsync(0);
    expect(fanOutEventStreamStatus).toHaveBeenCalledWith('open');
    expect(restoreLogs().slice(-1)[0]).toEqual(['restore-admission', 'admitted_after_stall',
      { waited_ms: RESTORE_ADMISSION_STALL_MS }]);
  });

  it('costs a healthy admission no restart and no log line', async () => {
    connectThreadEvents();
    opened!.handlers.onOpen();
    opened!.handlers.onFrame(JSON.stringify({ type: 'StreamHello', data: { restore_generation: null } }));
    await vi.waitFor(() => expect(fanOutEventStreamStatus).toHaveBeenCalledWith('open'));
    admissionPhase = 'awaiting-hello';
    await vi.advanceTimersByTimeAsync(RESTORE_ADMISSION_STALL_MS * 3);
    expect(opened!.stream.restart).not.toHaveBeenCalled();
    expect(restoreLogs()).toEqual([]);
  });

  // The retry's new epoch resets the phase, so the failure names it first.
  it('names the step a failed hello stopped in', async () => {
    admissionPhase = 'journal';
    acceptRestoreHello.mockRejectedValueOnce(new Error('storage stalled'));
    connectThreadEvents();
    const first = opened!;
    first.handlers.onOpen();
    first.handlers.onFrame(JSON.stringify({ type: 'StreamHello', data: { restore_generation: null } }));
    await vi.waitFor(() => expect(first.stream.close).toHaveBeenCalled());
    expect(restoreLogs()).toEqual([['restore-admission', 'failed',
      { phase: 'journal', error: 'storage stalled' }]]);
  });

  /** Opens a connection whose hello fails with `error`, and waits for it to close. */
  async function failHello(error: Error) {
    acceptRestoreHello.mockRejectedValueOnce(error);
    if (!opened || opened.stream.close.mock.calls.length > 0) await vi.advanceTimersByTimeAsync(10_000);
    const connection = opened!;
    connection.handlers.onOpen();
    connection.handlers.onFrame(JSON.stringify({ type: 'StreamHello', data: { restore_generation: null } }));
    await vi.waitFor(() => expect(connection.stream.close).toHaveBeenCalled());
  }
  const stalled = () => new DeviceDatabaseStalledError('lucidos-restore-generation:ws', 'transaction');

  it('retries a device storage stall without a toast', async () => {
    opened = null;
    connectThreadEvents();
    await failHello(stalled());
    expect(showToast).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(10_000);
    expect(openEventStream).toHaveBeenCalledTimes(2);
    opened!.handlers.onOpen();
    opened!.handlers.onFrame(JSON.stringify({ type: 'StreamHello', data: { restore_generation: null } }));
    await vi.waitFor(() => expect(fanOutEventStreamStatus).toHaveBeenCalledWith('open'));
    expect(showToast).not.toHaveBeenCalled();
  });

  it('says nothing about steps released when the page hid, and does not count them as stalls', async () => {
    opened = null;
    connectThreadEvents();
    await failHello(new DeviceDatabaseReleasedError('lucidos-restore-generation:ws'));
    await failHello(new DeviceDatabaseReleasedError('lucidos-restore-generation:ws'));
    await failHello(stalled());
    expect(showToast).not.toHaveBeenCalled();
    // Admit, which resets the stall count the next case starts from.
    await vi.advanceTimersByTimeAsync(10_000);
    opened!.handlers.onOpen();
    opened!.handlers.onFrame(JSON.stringify({ type: 'StreamHello', data: { restore_generation: null } }));
    await vi.waitFor(() => expect(fanOutEventStreamStatus).toHaveBeenCalledWith('open'));
  });

  it('offers Retry once device storage keeps stalling, and clears it on admission', async () => {
    opened = null;
    connectThreadEvents();
    await failHello(stalled());
    await failHello(stalled());
    expect(showToast).toHaveBeenCalledOnce();
    const [message, type, opts] = vi.mocked(showToast).mock.calls[0];
    expect(type).toBe('warning');
    expect(message).not.toContain('lucidos-restore-generation');
    expect(message).not.toMatch(/restore|corrupt|lost/i);
    expect(opts).toMatchObject({ title: 'Device storage', key: expect.any(String) });

    // Retry reconnects now, through a new connection epoch.
    opts!.action!.onClick();
    expect(openEventStream).toHaveBeenCalledTimes(3);
    opened!.handlers.onOpen();
    opened!.handlers.onFrame(JSON.stringify({ type: 'StreamHello', data: { restore_generation: null } }));
    await vi.waitFor(() => expect(fanOutEventStreamStatus).toHaveBeenCalledWith('open'));
    expect(removeToast).toHaveBeenCalledWith(opts!.key);
    await vi.advanceTimersByTimeAsync(10_000);
    expect(openEventStream).toHaveBeenCalledTimes(3);
  });

  it('restarts an attempt still waiting on storage when Retry is tapped', async () => {
    opened = null;
    connectThreadEvents();
    await failHello(stalled());
    await failHello(stalled());
    const [, , opts] = vi.mocked(showToast).mock.calls[0];
    // The reconnect timer has fired, and its hello waits on storage forever.
    await vi.advanceTimersByTimeAsync(10_000);
    const waiting = opened!;
    acceptRestoreHello.mockReturnValueOnce(new Promise<boolean>(() => {}));
    waiting.handlers.onOpen();
    waiting.handlers.onFrame(JSON.stringify({ type: 'StreamHello', data: { restore_generation: null } }));
    await vi.waitFor(() => expect(acceptRestoreHello).toHaveBeenCalledTimes(3));
    expect(openEventStream).toHaveBeenCalledTimes(3);

    opts!.action!.onClick();
    expect(waiting.stream.close).toHaveBeenCalled();
    expect(openEventStream).toHaveBeenCalledTimes(4);
    opened!.handlers.onOpen();
    opened!.handlers.onFrame(JSON.stringify({ type: 'StreamHello', data: { restore_generation: null } }));
    await vi.waitFor(() => expect(fanOutEventStreamStatus).toHaveBeenCalledWith('open'));
    expect(removeToast).toHaveBeenCalledWith(opts!.key);
  });

  it('starts counting storage stalls again after an admission', async () => {
    opened = null;
    connectThreadEvents();
    await failHello(stalled());
    await vi.advanceTimersByTimeAsync(10_000);
    opened!.handlers.onOpen();
    opened!.handlers.onFrame(JSON.stringify({ type: 'StreamHello', data: { restore_generation: null } }));
    await vi.waitFor(() => expect(fanOutEventStreamStatus).toHaveBeenCalledWith('open'));
    opened!.handlers.onError(ERROR);
    await failHello(stalled());
    expect(showToast).not.toHaveBeenCalled();
  });

  it('still reports a failure that is not a storage stall', async () => {
    opened = null;
    connectThreadEvents();
    await failHello(new Error('storage full'));
    expect(showToast).toHaveBeenCalledWith(expect.stringContaining('storage full'), 'error', { title: 'Workspace recovery' });
  });

  it.each([
    ['cancelled by a backgrounded page', new DOMException('Fetch is aborted', 'AbortError')],
    ['timed out', new DOMException('Request timed out after 10000ms', 'TimeoutError')],
    ['dropped by the transport', new TypeError('Load failed')],
  ])('retries a hello whose fetch was %s without a toast', async (_, err) => {
    opened = null;
    connectThreadEvents();
    await failHello(err);
    expect(showToast).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(10_000);
    expect(openEventStream).toHaveBeenCalledTimes(2);
  });
});

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

  it('opens a direct remote stream without requesting a foreign shared worker', () => {
    remoteGateway = 'cloud';
    connectThreadEvents();
    expect(openDirectEventStream).toHaveBeenCalledOnce();
    expect(openedTargets?.streamUrl).toBe('/api/v1/events');
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

    expect(processSSEForReferences).toHaveBeenCalledWith('NotificationCreated', { id: 'n-1' }, undefined);
  });

  it('passes stable system identity through the production host dispatch without changing target ids', () => {
    connectThreadEvents();
    const data = { event_id: 'tap-target', provider: 'example' };
    for (const event_id of ['request-one', 'request-one', 'request-two']) {
      opened!.handlers.onFrame(JSON.stringify({ type: 'OAuthAccountConnected', event_id, data }));
    }
    expect(processSSEForReferences.mock.calls.map((call) => call[2]))
      .toEqual(['request-one', 'request-one', 'request-two']);
    expect(processSSEForReferences).toHaveBeenCalledWith('OAuthAccountConnected', data, 'request-one');
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
      expect(processSSEForReferences).toHaveBeenCalledWith('NotificationCreated', { id: 'n-2' }, undefined);
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
      expect(processSSEForReferences).not.toHaveBeenCalledWith('NotificationCreated', { id: 'n-3' }, undefined);
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

  it('invalidates version status on the first open and reconnect', () => {
    connectThreadEvents();
    const before = engineVersionStatusVersion.value;
    opened?.handlers.onOpen();
    expect(engineVersionStatusVersion.value).toBe(before + 1);
    const firstOpen = engineVersionStatusVersion.value;
    opened?.handlers.onError(ERROR);
    opened?.handlers.onOpen();
    expect(engineVersionStatusVersion.value).toBe(firstOpen + 1);
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

  it('refreshes form eligibility on archive and unarchive without reconnecting', () => {
    connectThreadEvents();
    for (const type of ['ThreadArchived', 'ThreadUnarchived']) {
      opened?.handlers.onFrame(JSON.stringify({
        type: 'ThreadEvent',
        data: { thread_id: 'archive-form-thread', seq: null, event: { type } },
      }));
    }
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

  it('posts one log line when the shared worker forgot the page or stopped answering', () => {
    // Without these lines a lost worker reads exactly like a healthy one.
    connectThreadEvents();
    opened?.handlers.onOpen();

    opened?.handlers.onError({ reason: 'worker-restarted' });
    opened?.handlers.onError({ reason: 'worker-unresponsive' });

    expect(postClientLog.mock.calls).toEqual([
      ['event-stream', 'worker-restarted', {}],
      ['event-stream', 'worker-unresponsive', {}],
    ]);
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


describe('cold store dispatch behind eager restore admission', () => {
  beforeEach(() => {
    disconnectThreadEvents();
    vi.clearAllMocks();
    idleSinks.length = 0;
    sinkLoaded = false;
    sinkPreload.mockReset().mockResolvedValue(true);
    nextOwnsReconnect = true;
    threadMap.value = new Map();
    focusedThreadId.value = null;
  });
  afterEach(() => { disconnectThreadEvents(); sinkLoaded = true; });
  const frame = (id: string) => JSON.stringify({ type: 'NotificationCreated', event_id: id, data: { id } });

  it('retains early frames in arrival order and does not load before idle', async () => {
    connectThreadEvents();
    opened!.handlers.onOpen();
    opened!.handlers.onFrame(frame('first'));
    opened!.handlers.onFrame(frame('second'));
    expect(processSSEForReferences).not.toHaveBeenCalled();
    expect(sinkPreload).not.toHaveBeenCalled();
    expect(fanOutEventFrame.mock.calls.map((call) => JSON.parse(call[0]).event_id)).toEqual(['first', 'second']);
    await idleSinks[idleSinks.length - 1]!.preload();
    expect(fanOutEventFrame).toHaveBeenCalledTimes(2);
    expect(processSSEForReferences.mock.calls.map((call) => call[2])).toEqual(['first', 'second']);
  });

  it('keeps admitted frames across a routine reconnect while the chunk waits', async () => {
    connectThreadEvents();
    opened!.handlers.onOpen();
    opened!.handlers.onFrame(frame('before-reconnect'));
    opened!.handlers.onError(ERROR);
    opened!.handlers.onOpen();
    opened!.handlers.onFrame(frame('after-reconnect'));
    await idleSinks[idleSinks.length - 1]!.preload();
    expect(processSSEForReferences.mock.calls.map((call) => call[2])).toEqual(['before-reconnect', 'after-reconnect']);
  });

  it('keeps admitted frames when a dedicated transport reconnects before idle', async () => {
    vi.useFakeTimers();
    try {
      nextOwnsReconnect = false;
      connectThreadEvents();
      opened!.handlers.onOpen();
      opened!.handlers.onFrame(frame('before-replacement'));
      opened!.handlers.onError(ERROR);
      await vi.advanceTimersByTimeAsync(10_000);
      opened!.handlers.onOpen();
      opened!.handlers.onFrame(frame('after-replacement'));
      await idleSinks[idleSinks.length - 1]!.preload();
      expect(processSSEForReferences.mock.calls.map((call) => call[2])).toEqual(['before-replacement', 'after-replacement']);
    } finally { vi.useRealTimers(); }
  });

  it('keeps admitted frames when wake replaces a stale stream before idle', async () => {
    nextHealth = 'stale';
    connectThreadEvents();
    opened!.handlers.onFrame(frame('before-wake'));
    reviveThreadEvents();
    opened!.handlers.onFrame(frame('after-wake'));
    await idleSinks[idleSinks.length - 1]!.preload();
    expect(processSSEForReferences.mock.calls.map((call) => call[2])).toEqual(['before-wake', 'after-wake']);
    nextHealth = 'live';
  });

  it('retains frames after a failed chunk fetch and Retry drains them once', async () => {
    sinkPreload.mockResolvedValueOnce(false);
    connectThreadEvents();
    opened!.handlers.onFrame(frame('waiting'));
    await idleSinks[idleSinks.length - 1]!.preload();
    expect(processSSEForReferences).not.toHaveBeenCalled();
    expect(fanOutEventFrame).toHaveBeenCalledWith(frame('waiting'));
    const options = vi.mocked(showToast).mock.calls[vi.mocked(showToast).mock.calls.length - 1]![2]!;
    expect(options.key).toBe('event-dispatch-error');
    options.action!.onClick();
    await vi.waitFor(() => expect(processSSEForReferences).toHaveBeenCalledTimes(1));
    expect(processSSEForReferences.mock.calls[0][2]).toBe('waiting');
  });

  it('rejects late dispatch from a disconnected connection', async () => {
    let finish!: (loaded: boolean) => void;
    sinkPreload.mockImplementationOnce(() => new Promise<boolean>((resolve) => { finish = resolve; }));
    connectThreadEvents();
    opened!.handlers.onFrame(frame('stale'));
    const loading = idleSinks[idleSinks.length - 1]!.preload();
    disconnectThreadEvents();
    finish(true);
    await loading;
    expect(processSSEForReferences).not.toHaveBeenCalled();
  });

  it('admits restore hello without waiting for the dispatcher chunk', async () => {
    restoreRuntimeActive = true;
    connectThreadEvents();
    opened!.handlers.onOpen();
    opened!.handlers.onFrame(JSON.stringify({ type: 'StreamHello', data: { restore_generation: null } }));
    opened!.handlers.onFrame(frame('admitted'));
    await vi.waitFor(() => expect(fanOutEventStreamStatus).toHaveBeenCalledWith('open'));
    expect(acceptRestoreHello).toHaveBeenCalled();
    expect(sinkPreload).not.toHaveBeenCalled();
    expect(processSSEForReferences).not.toHaveBeenCalled();
    await idleSinks[idleSinks.length - 1]!.preload();
    expect(processSSEForReferences.mock.calls.map((call) => call[2])).toEqual(['admitted']);
  });
});
