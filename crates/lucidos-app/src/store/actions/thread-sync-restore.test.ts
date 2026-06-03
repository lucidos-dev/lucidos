import { describe, it, expect, beforeEach, vi } from 'vitest';
import { signal } from '@preact/signals-core';
import type { RestoreState } from '../../api/client';

const restoreState = signal<RestoreState | null>(null);
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
  ccSessionVersion: signal(0),
  memoryRebuildProgress: signal(null),
  backupProgress: signal(null),
  restoreState,
  backupListVersion: signal(0),
  backupStatusVersion: signal(0),
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

// The Restore* SSE events must land restoreState in the SAME shape that
// getRestoreStatus() returns, so a live page and a reloaded page render
// identically. These tests pin that mapping.
describe('handleGlobalEvent — Restore events', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    restoreState.value = null;
  });

  it('RestoreProgress sets a running state mirroring the tick', () => {
    handleGlobalEvent('RestoreProgress', {
      workspace_name: 'personal',
      phase: 'downloading',
      progress: 12,
      total: 100,
    });

    expect(restoreState.value).toEqual({
      status: 'running',
      workspace_name: 'personal',
      phase: 'downloading',
      progress: 12,
      total: 100,
    });
    // Progress alone doesn't toast.
    expect(showToast).not.toHaveBeenCalled();
  });

  it('RestoreCompleted sets a completed state and toasts success', () => {
    handleGlobalEvent('RestoreCompleted', {
      workspace_name: 'personal',
      workspace_path: '/Users/me/workspaces/personal',
    });

    expect(restoreState.value).toEqual({
      status: 'completed',
      workspace_name: 'personal',
      workspace_path: '/Users/me/workspaces/personal',
    });
    expect(showToast).toHaveBeenCalledExactlyOnceWith(
      'Workspace restored: personal',
      'success',
    );
  });

  it('RestoreFailed sets a failed state and toasts the error', () => {
    handleGlobalEvent('RestoreFailed', {
      workspace_name: 'personal',
      error: 'pg_restore exited 1',
    });

    expect(restoreState.value).toEqual({
      status: 'failed',
      workspace_name: 'personal',
      error: 'pg_restore exited 1',
    });
    expect(showToast).toHaveBeenCalledExactlyOnceWith(
      'Restore failed: pg_restore exited 1',
      'error',
    );
  });

  it('RestoreFailed with no error field falls back to "Unknown error"', () => {
    handleGlobalEvent('RestoreFailed', { workspace_name: 'personal' });

    expect(restoreState.value).toEqual({
      status: 'failed',
      workspace_name: 'personal',
      error: 'Unknown error',
    });
    expect(showToast).toHaveBeenCalledExactlyOnceWith(
      'Restore failed: Unknown error',
      'error',
    );
  });
});
