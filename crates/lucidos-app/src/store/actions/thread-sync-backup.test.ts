import { describe, it, expect, beforeEach, vi } from 'vitest';
import { signal } from '@preact/signals-core';

const backupProgress = signal<{ phase: string; progress: number; total: number } | null>(null);
const backupStatusVersion = signal(0);
const showToast = vi.fn();

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
}));
vi.mock('./navigation', () => ({ pushNavState: vi.fn() }));
vi.mock('./push', () => ({ initPushSubscription: vi.fn() }));
vi.mock('./devices', () => ({ getDeviceId: vi.fn(), toggleDevicePush: vi.fn() }));
vi.mock('../../components/chat/scrollState', () => ({ scrollToBottom: vi.fn() }));
vi.mock('./threads', () => ({ focusThread: vi.fn() }));
vi.mock('./repositories', () => ({ refreshRepoView: vi.fn() }));
vi.mock('./entityReferences', () => ({ processSSEForReferences: vi.fn() }));
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
      filename: 'lucidos-backup-personal-20260504-090000.enc',
      size_bytes: 927_401_289,
    });

    expect(backupProgress.value).toBeNull();
    expect(backupStatusVersion.value).toBe(1);
    expect(showToast).toHaveBeenCalledExactlyOnceWith(
      'Backup created: lucidos-backup-personal-20260504-090000.enc (884 MB)',
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
    );
  });

  it('BackupFailed with no error field falls back to "Unknown error"', () => {
    handleGlobalEvent('BackupFailed', {});

    expect(showToast).toHaveBeenCalledExactlyOnceWith(
      'Backup failed: Unknown error',
      'error',
    );
  });
});
