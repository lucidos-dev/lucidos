import { describe, it, expect, beforeEach, vi } from 'vitest';
import { signal } from '@preact/signals-core';

// The per-thread half of the side-effect we're verifying.
const refreshThreadEvents = vi.fn(async (_id: string) => {});
const markLoadedThreadsStale = vi.fn();
vi.mock('./thread-loading', () => ({ refreshThreadEvents, markLoadedThreadsStale }));

// The metadata read goes through the shared wrapper, which is what owns the one
// keyed card this shares with the resume sync (see thread-list-refresh.test.ts
// for the reporting rules themselves). Mocked here so this suite stays about
// resync ORDERING; asserting on it also pins that the site still routes through
// the wrapper rather than calling `loadAllThreads` directly and growing its own
// catch block, which is the divergence the shared module exists to prevent.
const refreshThreadList = vi.fn(async () => {});
vi.mock('./thread-list-refresh', () => ({ refreshThreadList }));

// Stub the threadMap + focusedThreadId signals so the test can set fixture state
// without pulling in the full store graph (which depends on browser-only modules).
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

// Side-effect imports thread-sync.ts pulls in — kept as no-ops.
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
vi.mock('./entityReferences', () => ({
  processSSEForReferences: vi.fn(),
  refreshLlmConfigured: vi.fn(),
  PROVIDER_PREFERENCE_KEYS: new Set(['opencode_free_enabled', 'provider_enabled_openai']),
}));

const { resyncLoadedThreads } = await import('./thread-sync');

describe('resyncLoadedThreads', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    threadMap.value = new Map();
    focusedThreadId.value = null;
  });

  it('refreshes thread metadata, then the focused thread only', async () => {
    threadMap.value = new Map([
      ['thread-a', { meta: { id: 'thread-a' }, eventsLoaded: true }],
      ['thread-b', { meta: { id: 'thread-b' }, eventsLoaded: false }],
      ['thread-c', { meta: { id: 'thread-c' }, eventsLoaded: true }],
    ]);
    focusedThreadId.value = 'thread-c';

    await resyncLoadedThreads();

    // The metadata read is what repairs the drawer after an SSE gap (the stuck
    // "Thinking" spinner this function exists for reads `meta.status`), and it
    // covers every thread in one request. Only the transcript on screen needs
    // its events now; the rest are marked and catch up on focus.
    expect(refreshThreadList).toHaveBeenCalledTimes(1);
    expect(markLoadedThreadsStale).toHaveBeenCalledTimes(1);
    expect(refreshThreadEvents.mock.calls.map((c) => c[0])).toEqual(['thread-c']);
  });

  it('marks without refreshing anything when no thread is focused', async () => {
    // An SSE drop reconnects every 3s and each reopen ran this. On a workspace
    // this size, unbounded, it put every request on one connection and one radio
    // wake, racing the same 10s client deadline down a link that had only just
    // come back; bounded, it queued the same burst. Now there is no burst.
    threadMap.value = new Map(
      Array.from({ length: 20 }, (_, i) => [`t${i}`, { meta: { id: `t${i}` }, eventsLoaded: true }] as const),
    );

    await resyncLoadedThreads();

    expect(refreshThreadList).toHaveBeenCalledTimes(1);
    expect(markLoadedThreadsStale).toHaveBeenCalledTimes(1);
    expect(refreshThreadEvents).not.toHaveBeenCalled();
  });

  it('does not refresh a focused thread whose events never loaded', async () => {
    // `refreshThreadEvents` would decline it (there is no `lastDbSeq` to fetch
    // after); the full load is `focusThread`'s / the retry's business.
    threadMap.value = new Map([
      ['thread-a', { meta: { id: 'thread-a' }, eventsLoaded: false }],
    ]);
    focusedThreadId.value = 'thread-a';

    await resyncLoadedThreads();

    expect(refreshThreadEvents).not.toHaveBeenCalled();
  });

  it('coalesces concurrent calls into one network round-trip', async () => {
    threadMap.value = new Map([
      ['thread-a', { meta: { id: 'thread-a' }, eventsLoaded: true }],
    ]);
    focusedThreadId.value = 'thread-a';

    // Three callers race — only one resync should actually run.
    await Promise.all([
      resyncLoadedThreads(),
      resyncLoadedThreads(),
      resyncLoadedThreads(),
    ]);

    expect(refreshThreadList).toHaveBeenCalledTimes(1);
    expect(markLoadedThreadsStale).toHaveBeenCalledTimes(1);
    expect(refreshThreadEvents).toHaveBeenCalledTimes(1);
  });
});
