import { describe, it, expect, beforeEach, vi } from 'vitest';
import { signal } from '@preact/signals-core';

const backupProgress = signal<{ phase: string; progress: number; total: number } | null>(null);
const backupStatusVersion = signal(0);
const showToast = vi.fn();
const dismissToast = vi.fn();
const openBackupSettings = vi.fn();

vi.mock('../store', () => ({
  threadMap: signal(new Map()),
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
  backupProgress,
  backupStatusVersion,
  backupPreferencesVersion: signal(0),
  recoveryProgress: signal(null),
  panelOverlay: signal(null),
  showConfirm: vi.fn(),
  showToast,
  dismissToast,
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
  openBackupSettings,
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

describe('handleGlobalEvent — Backup terminal events', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    backupProgress.value = { phase: 'encrypting', progress: 60, total: 100 };
    backupStatusVersion.value = 0;
  });

  it('BackupCompleted clears progress, toasts once, and bumps the status version', () => {
    handleGlobalEvent('BackupCompleted', {
      filename: 'lucidos-backup-myws-20260504-090000.enc',
      size_bytes: 927_401_289,
    });

    expect(backupProgress.value).toBeNull();
    expect(backupStatusVersion.value).toBe(1);
    expect(showToast).toHaveBeenCalledExactlyOnceWith(
      'Backup created: lucidos-backup-myws-20260504-090000.enc (884 MB)',
      'success',
    );
  });

  it('BackupFailed clears progress, toasts the error, and bumps the status version', () => {
    handleGlobalEvent('BackupFailed', {
      error: 'Token refresh failed (invalid_grant)',
    });

    expect(backupProgress.value).toBeNull();
    expect(backupStatusVersion.value).toBe(1);
    expect(showToast).toHaveBeenCalledExactlyOnceWith(
      'Backup failed: Token refresh failed (invalid_grant)',
      'error',
      expect.objectContaining({ key: 'backup-failed', onClick: expect.any(Function) }),
    );
  });

  it('BackupFailed with no error field falls back to "Unknown error"', () => {
    handleGlobalEvent('BackupFailed', {});

    expect(showToast).toHaveBeenCalledExactlyOnceWith(
      'Backup failed: Unknown error',
      'error',
      expect.objectContaining({ key: 'backup-failed' }),
    );
  });

  // The toast names a problem the Backup page fixes, so the tap goes there.
  it('tapping the BackupFailed toast dismisses it and opens the Backup page', () => {
    handleGlobalEvent('BackupFailed', { error: 'Google Drive is full' });

    const opts = showToast.mock.calls[0][2] as { onClick: () => void };
    opts.onClick();

    expect(dismissToast).toHaveBeenCalledExactlyOnceWith('backup-failed');
    expect(openBackupSettings).toHaveBeenCalledOnce();
  });
});

describe('handleGlobalEvent: ProxyConfigRejected', () => {
  beforeEach(() => vi.clearAllMocks());

  it('names every refused provider and its reason', () => {
    handleGlobalEvent('ProxyConfigRejected', {
      rejected: [
        { provider: 'jira', reason: "provider 'jira': auth uses the removed legacy shape" },
        { provider: 'binance', reason: 'unknown variant `md5`' },
      ],
    });

    expect(showToast).toHaveBeenCalledExactlyOnceWith(
      "Proxy config problem, jira: provider 'jira': auth uses the removed legacy shape; "
      + 'binance: unknown variant `md5`',
      'error',
      expect.objectContaining({ key: 'proxy-config-rejected', showWhileUnavailable: true }),
    );
  });

  it('reads a null provider as the file itself', () => {
    handleGlobalEvent('ProxyConfigRejected', {
      rejected: [{ provider: null, reason: 'is not usable: expected value at line 1' }],
    });

    expect(showToast).toHaveBeenCalledExactlyOnceWith(
      'Proxy config problem, data/config/apis.json: is not usable: expected value at line 1',
      'error',
      expect.objectContaining({ key: 'proxy-config-rejected' }),
    );
  });

  // A healthy workspace must stay silent, so nothing learns to ignore it.
  it('says nothing when the list is empty or absent', () => {
    handleGlobalEvent('ProxyConfigRejected', { rejected: [] });
    handleGlobalEvent('ProxyConfigRejected', {});
    expect(showToast).not.toHaveBeenCalled();
  });
});
