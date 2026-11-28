import { describe, it, expect, beforeEach, vi } from 'vitest';
import { signal } from '@preact/signals-core';

const refreshThreadEvents = vi.fn(async (_id: string) => {});
const markLoadedThreadsStale = vi.fn();
vi.mock('./thread-loading', () => ({ refreshThreadEvents, markLoadedThreadsStale }));

const refreshThreadList = vi.fn(async () => {});
vi.mock('./thread-list-refresh', () => ({ refreshThreadList }));

const threadMap = signal(new Map<string, { meta: { id: string }; eventsLoaded: boolean }>());
const focusedThreadId = signal<string | null>(null);
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
vi.mock('./devices', () => ({ getDeviceId: vi.fn() }));
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

    expect(refreshThreadList).toHaveBeenCalledTimes(1);
    expect(markLoadedThreadsStale).toHaveBeenCalledTimes(1);
    expect(refreshThreadEvents.mock.calls.map((c) => c[0])).toEqual(['thread-c']);
  });

  it('marks without refreshing anything when no thread is focused', async () => {
    threadMap.value = new Map(
      Array.from({ length: 20 }, (_, i) => [`t${i}`, { meta: { id: `t${i}` }, eventsLoaded: true }] as const),
    );

    await resyncLoadedThreads();

    expect(refreshThreadList).toHaveBeenCalledTimes(1);
    expect(markLoadedThreadsStale).toHaveBeenCalledTimes(1);
    expect(refreshThreadEvents).not.toHaveBeenCalled();
  });

  it('does not refresh a focused thread whose events never loaded', async () => {
    threadMap.value = new Map([
      ['thread-a', { meta: { id: 'thread-a' }, eventsLoaded: false }],
    ]);
    focusedThreadId.value = 'thread-a';

    await resyncLoadedThreads();

    expect(refreshThreadEvents).not.toHaveBeenCalled();
  });

  it('reruns once for a gap that opened while a resync ran', async () => {
    threadMap.value = new Map([
      ['thread-a', { meta: { id: 'thread-a' }, eventsLoaded: true }],
    ]);
    focusedThreadId.value = 'thread-a';
    const reads: Array<() => void> = [];
    const heldRead = () => new Promise<void>((resolve) => reads.push(resolve));
    refreshThreadList.mockImplementationOnce(heldRead).mockImplementationOnce(heldRead);

    void resyncLoadedThreads();
    let secondDone = false;
    const second = resyncLoadedThreads().then(() => { secondDone = true; });
    expect(refreshThreadList).toHaveBeenCalledTimes(1);

    reads[0]();
    await vi.waitFor(() => expect(refreshThreadList).toHaveBeenCalledTimes(2));
    expect(markLoadedThreadsStale).toHaveBeenCalledTimes(2);
    expect(secondDone).toBe(false);

    reads[1]();
    await second;
    expect(refreshThreadEvents).toHaveBeenCalledTimes(2);
  });

  it('folds every call that lands mid-resync into one trailing run', async () => {
    threadMap.value = new Map([
      ['thread-a', { meta: { id: 'thread-a' }, eventsLoaded: true }],
    ]);
    focusedThreadId.value = 'thread-a';

    await Promise.all([
      resyncLoadedThreads(),
      resyncLoadedThreads(),
      resyncLoadedThreads(),
    ]);

    expect(refreshThreadList).toHaveBeenCalledTimes(2);
    expect(markLoadedThreadsStale).toHaveBeenCalledTimes(2);
    expect(refreshThreadEvents).toHaveBeenCalledTimes(2);
  });

  it('starts a fresh resync, with no rerun, for a call after the last one ended', async () => {
    await resyncLoadedThreads();
    await resyncLoadedThreads();

    expect(refreshThreadList).toHaveBeenCalledTimes(2);
  });
});
