import { describe, it, expect, beforeEach, vi } from 'vitest';
import { signal } from '@preact/signals-core';

const applyingChangeIds = signal<Set<string>>(new Set());
const applyAllInProgress = signal(false);
const showToast = vi.fn();

vi.mock('../store', () => ({
  threadMap: signal(new Map()),
  focusedThreadId: signal<string | null>(null),
  changes: signal([]),
  appliedChanges: signal([]),
  changesHasMore: signal(false),
  updateAvailable: signal(false),
  applyingChangeIds,
  applyingNowThreadIds: signal(new Map()),
  applyAllInProgress,
  generatedTitleIds: new Set(),
  codingAgentSessionVersion: signal(0),
  memoryRebuildProgress: signal(null),
  backupProgress: signal(null),
  backupStatusVersion: signal(0),
  backupPreferencesVersion: signal(0),
  recoveryProgress: signal(null),
  panelOverlay: signal(null),
  showConfirm: vi.fn(),
  showToast,
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
vi.mock('./entityReferences', () => ({
  processSSEForReferences: vi.fn(),
  refreshLlmConfigured: vi.fn(),
  PROVIDER_PREFERENCE_KEYS: new Set(['opencode_free_enabled', 'provider_enabled_openai']),
}));
vi.mock('./thread-loading', () => ({
  loadAllThreads: vi.fn(async () => {}),
  refreshThreadEvents: vi.fn(async () => {}),
}));

const { handleGlobalEvent } = await import('./thread-sync');

describe('handleGlobalEvent — Apply All batch events', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    applyingChangeIds.value = new Set();
    applyAllInProgress.value = false;
  });

  it('ApplyAllBatchStarted flags in-progress and marks every member as applying', () => {
    handleGlobalEvent('ApplyAllBatchStarted', {
      batch_id: 'batch-1',
      change_ids: ['c1', 'c2', 'c3'],
    });

    expect(applyAllInProgress.value).toBe(true);
    expect(applyingChangeIds.value.has('c1')).toBe(true);
    expect(applyingChangeIds.value.has('c2')).toBe(true);
    expect(applyingChangeIds.value.has('c3')).toBe(true);
  });

  it('ApplyAllBatchStarted preserves change ids that were already applying', () => {
    applyingChangeIds.value = new Set(['existing']);

    handleGlobalEvent('ApplyAllBatchStarted', { batch_id: 'batch-1', change_ids: ['c1'] });

    expect(applyingChangeIds.value.has('existing')).toBe(true);
    expect(applyingChangeIds.value.has('c1')).toBe(true);
  });

  it('ApplyAllBatchCompleted clears the in-progress flag and all resolved member ids', () => {
    applyAllInProgress.value = true;
    applyingChangeIds.value = new Set(['c1', 'c2', 'c3']);

    handleGlobalEvent('ApplyAllBatchCompleted', { batch_id: 'batch-1', applied: ['c1', 'c2', 'c3'] });

    expect(applyAllInProgress.value).toBe(false);
    expect(applyingChangeIds.value.size).toBe(0);
  });

  it('ApplyAllBatchCompleted clears canceled (failed) members so a canceled batch leaves no stragglers', () => {
    // On cancel the queued members never emit a per-change event — they arrive
    // only in the Completed event's `failed` list with "Apply All canceled".
    applyAllInProgress.value = true;
    applyingChangeIds.value = new Set(['c1', 'c2', 'c3']);

    handleGlobalEvent('ApplyAllBatchCompleted', {
      batch_id: 'batch-1',
      applied: ['c1'],
      failed: [
        { change_id: 'c2', error: 'Apply All canceled' },
        { change_id: 'c3', error: 'Apply All canceled' },
      ],
    });

    expect(applyAllInProgress.value).toBe(false);
    expect(applyingChangeIds.value.size).toBe(0);
  });

  it('ApplyAllBatchCompleted leaves unrelated applying ids untouched', () => {
    applyAllInProgress.value = true;
    applyingChangeIds.value = new Set(['c1', 'unrelated']);

    handleGlobalEvent('ApplyAllBatchCompleted', { batch_id: 'batch-1', applied: ['c1'] });

    expect(applyingChangeIds.value.has('unrelated')).toBe(true);
    expect(applyingChangeIds.value.has('c1')).toBe(false);
  });
});
