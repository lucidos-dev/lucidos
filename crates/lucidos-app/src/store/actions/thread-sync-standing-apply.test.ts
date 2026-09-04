import { describe, it, expect, beforeEach, vi } from 'vitest';
import { signal } from '@preact/signals-core';

const applyingChangeIds = signal<Set<string>>(new Set());
const applyAllInProgress = signal(false);
const standingApplyThreadIds = signal<Set<string>>(new Set());
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
  standingApplyThreadIds,
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
vi.mock('./chat-changes', () => ({
  addRestartGroup: vi.fn(),
  STANDING_APPLY_CANCELED: 'Canceled.',
}));
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

describe('handleGlobalEvent: standing apply', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    standingApplyThreadIds.value = new Set();
  });

  it('StandingApplyArmed adds the thread, so every surface draws the armed face', () => {
    handleGlobalEvent('StandingApplyArmed', { thread_id: 't1', change_id: 'c1' });
    expect(standingApplyThreadIds.value.has('t1')).toBe(true);
  });

  it('StandingApplyArmed keeps threads armed by another press', () => {
    standingApplyThreadIds.value = new Set(['t0']);
    handleGlobalEvent('StandingApplyArmed', { thread_id: 't1' });
    expect([...standingApplyThreadIds.value].sort()).toEqual(['t0', 't1']);
  });

  it('StandingApplyDropped clears the thread and reports why', () => {
    standingApplyThreadIds.value = new Set(['t1']);
    handleGlobalEvent('StandingApplyDropped', {
      thread_id: 't1',
      reason: 'The thread parked on a question.',
    });
    expect(standingApplyThreadIds.value.has('t1')).toBe(false);
    expect(showToast).toHaveBeenCalledOnce();
    expect(String(showToast.mock.calls[0][0])).toContain('parked on a question');
  });

  it('says nothing when the owner cancelled it, which they already saw', () => {
    standingApplyThreadIds.value = new Set(['t1']);
    handleGlobalEvent('StandingApplyDropped', { thread_id: 't1', reason: 'Canceled.' });
    expect(standingApplyThreadIds.value.has('t1')).toBe(false);
    expect(showToast).not.toHaveBeenCalled();
  });

  // The workspace-scope off drops each arm on its own. So the panel and every
  // armed thread's prompt row clear together, with no refetch and no toast.
  it('the workspace off clears every thread, one drop at a time', () => {
    standingApplyThreadIds.value = new Set(['t1', 't2', 't3']);
    for (const thread_id of ['t1', 't2', 't3']) {
      handleGlobalEvent('StandingApplyDropped', { thread_id, reason: 'Canceled.' });
    }
    expect(standingApplyThreadIds.value.size).toBe(0);
    expect(showToast).not.toHaveBeenCalled();
  });
});
