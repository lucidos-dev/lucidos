import { describe, it, expect, beforeEach, vi } from 'vitest';
import { signal } from '@preact/signals-core';

// thread-loading is the side-effect we're verifying — capture both calls.
const loadAllThreads = vi.fn(async () => {});
const refreshThreadEvents = vi.fn(async (_id: string) => {});
vi.mock('./thread-loading', () => ({ loadAllThreads, refreshThreadEvents }));

// Stub the threadMap signal so the test can set fixture state without pulling
// in the full store graph (which depends on browser-only modules).
const threadMap = signal(new Map<string, { meta: { id: string }; eventsLoaded: boolean }>());
vi.mock('../store', () => ({
  threadMap,
  focusedThreadId: signal<string | null>(null),
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
}));
vi.mock('./navigation', () => ({ pushNavState: vi.fn(), replaceNavState: vi.fn() }));
vi.mock('./push', () => ({ initPushSubscription: vi.fn() }));
vi.mock('./devices', () => ({ getDeviceId: vi.fn(), toggleDevicePush: vi.fn() }));
vi.mock('../../components/chat/scrollState', () => ({ scrollToBottom: vi.fn() }));
vi.mock('./threads', () => ({ focusThread: vi.fn() }));
vi.mock('./repositories', () => ({ refreshRepoView: vi.fn(), openEncodedRepoFilePreview: vi.fn(() => false) }));
vi.mock('./entityReferences', () => ({ processSSEForReferences: vi.fn() }));

const { resyncLoadedThreads } = await import('./thread-sync');

describe('resyncLoadedThreads', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    threadMap.value = new Map();
  });

  it('refreshes thread metadata and calls refreshThreadEvents for each loaded thread', async () => {
    threadMap.value = new Map([
      ['thread-a', { meta: { id: 'thread-a' }, eventsLoaded: true }],
      ['thread-b', { meta: { id: 'thread-b' }, eventsLoaded: false }],
      ['thread-c', { meta: { id: 'thread-c' }, eventsLoaded: true }],
    ]);

    await resyncLoadedThreads();

    expect(loadAllThreads).toHaveBeenCalledTimes(1);
    expect(refreshThreadEvents).toHaveBeenCalledTimes(2);
    const refreshedIds = refreshThreadEvents.mock.calls.map((c) => c[0]).sort();
    expect(refreshedIds).toEqual(['thread-a', 'thread-c']);
  });

  it('coalesces concurrent calls into one network round-trip', async () => {
    threadMap.value = new Map([
      ['thread-a', { meta: { id: 'thread-a' }, eventsLoaded: true }],
    ]);

    // Three callers race — only one resync should actually run.
    await Promise.all([
      resyncLoadedThreads(),
      resyncLoadedThreads(),
      resyncLoadedThreads(),
    ]);

    expect(loadAllThreads).toHaveBeenCalledTimes(1);
    expect(refreshThreadEvents).toHaveBeenCalledTimes(1);
  });
});
