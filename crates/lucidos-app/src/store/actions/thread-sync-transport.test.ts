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

const refreshThreadEvents = vi.fn(async (_id: string) => {});
const markLoadedThreadsStale = vi.fn();
vi.mock('./thread-loading', () => ({ refreshThreadEvents, markLoadedThreadsStale }));

const refreshThreadList = vi.fn(async () => {});
vi.mock('./thread-list-refresh', () => ({ refreshThreadList }));

/** The transport the shell opened, captured so the test can drive its
 *  handlers. Standing in for a browser the unit test does not have. */
let opened: {
  handlers: { onFrame: (d: string) => void; onOpen: () => void; onError: () => void };
  opts: { pongs: boolean };
  stream: { close: ReturnType<typeof vi.fn>; ownsReconnect: boolean; submitPong: ReturnType<typeof vi.fn> };
} | null = null;

/** Whether the next transport claims to retry for itself. */
let nextOwnsReconnect = false;

/** The targets the shell asked for, so the URL construction is covered too. */
let openedTargets: { streamUrl: string; pongUrl: string; workerUrl: string } | null = null;

const openEventStream = vi.fn((targets, handlers, opts) => {
  const stream = { close: vi.fn(), ownsReconnect: nextOwnsReconnect, submitPong: vi.fn() };
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

const threadMap = signal(new Map<string, { meta: { id: string }; eventsLoaded: boolean }>());
const focusedThreadId = signal<string | null>(null);
vi.mock('../store', () => ({
  threadMap,
  focusedThreadId,
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
vi.mock('./notifications', () => ({ handleNotificationSSE: vi.fn() }));
vi.mock('./chat-changes', () => ({ addRestartGroup: vi.fn() }));
vi.mock('./preferences', () => ({ loadPreferences: vi.fn() }));
vi.mock('./artifacts', () => ({
  loadArtifacts: vi.fn(),
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
vi.mock('./devices', () => ({ getDeviceId: vi.fn(), pendingDeviceRegistration: vi.fn() }));
vi.mock('../../components/chat/scrollState', () => ({ followSentMessage: vi.fn(), stopFollowingBottom: vi.fn() }));
vi.mock('./threads', () => ({ focusThread: vi.fn() }));
vi.mock('./repositories', () => ({ refreshRepoView: vi.fn(), openEncodedRepoFilePreview: vi.fn(() => false) }));

const processSSEForReferences = vi.fn();
vi.mock('./entityReferences', () => ({
  processSSEForReferences,
  refreshLlmConfigured: vi.fn(),
  PROVIDER_PREFERENCE_KEYS: new Set(['opencode_free_enabled', 'provider_enabled_openai']),
}));

const { connectThreadEvents, disconnectThreadEvents } = await import('./thread-sync');

/** What the shell reports to the DOM, which is the only thing telling a user
 *  the stream is down. */
const status = () => document.documentElement.dataset.lucidosEventStream;

describe('the shell attaching to a transport', () => {
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

    opened?.handlers.onError();
    expect(status()).toBe('disconnected');
  });

  it('reconciles on the open that follows an error, not on the first one', async () => {
    // The first open is a page load, where useStartup has already read state.
    // Every later one follows a gap whose frames nobody replayed.
    connectThreadEvents();
    opened?.handlers.onOpen();
    expect(refreshThreadList).not.toHaveBeenCalled();

    opened?.handlers.onError();
    opened?.handlers.onOpen();
    await vi.runAllTimersAsync();

    expect(refreshThreadList).toHaveBeenCalledTimes(1);
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

    opened?.handlers.onError();
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

    opened?.handlers.onError();

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

    opened?.handlers.onError();
    expect(status()).toBe('disconnected');

    opened?.handlers.onOpen();
    await vi.runAllTimersAsync();

    expect(status()).toBe('connected');
    expect(refreshThreadList).toHaveBeenCalledTimes(1);
  });
});
