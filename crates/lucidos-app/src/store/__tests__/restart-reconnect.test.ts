import { engineVersionStatusVersion } from '../engineVersionStatus';
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { connectionStatus, toasts, restartRequired, restartGroups, engineStartedAt, updateAvailable, latestTauriAppVersion, engineVersion, latestEngineVersion, engineRestarting, engineRestartNewVersion, activeProgressDialog } from '../store';
import { syncRestartState, RESTART_LS_KEY } from '../actions/chat-changes';

const mockCheckHealth = vi.fn();
vi.mock('../../api/client', () => ({
  checkHealth: (...args: any[]) => mockCheckHealth(...args),
  API_BASE: 'http://localhost:3000',
  API: 'http://localhost:3000/api/v1',
}));
vi.mock('../actions/thread-stream', () => ({
  connectThreadEvents: vi.fn(),
  disconnectThreadEvents: vi.fn(),
  reviveThreadEvents: vi.fn(),
}));
vi.mock('../actions/thread-loading', () => ({
  loadAllThreads: vi.fn().mockResolvedValue(undefined),
  refreshThreadEvents: vi.fn().mockResolvedValue(true),
  loadThreadEvents: vi.fn().mockResolvedValue(undefined),
  clearThreadFetchGuards: vi.fn(),
  markLoadedThreadsStale: vi.fn(),
}));
vi.mock('../actions/chat-changes', async () => {
  const actual = await vi.importActual('../actions/chat-changes') as any;
  return {
    ...actual,
    refreshChangesState: vi.fn(),
  };
});
vi.mock('../actions/notifications', () => ({
  loadUnreadNotifications: vi.fn(),
}));
vi.mock('../actions/releaseNotices', () => ({
  loadReleaseNotices: vi.fn(),
}));

const { checkConnection, handleRestartTimeout } = await import('../actions/connection');
const { loadReleaseNotices } = await import('../actions/releaseNotices');
const { releaseNoticeDismissed } = await import('../releaseNotices');

const RESTART_FAILURE_TOAST_KEY = 'restart-required';
const STARTED_AT = '2026-03-20T06:00:00Z';
const RESTARTED_AT = '2026-03-20T07:00:00Z';

beforeEach(() => {
  vi.clearAllMocks();
  connectionStatus.value = 'connected';
  engineStartedAt.value = null;
  restartRequired.value = false;
  restartGroups.value = [];
  updateAvailable.value = false;
  latestTauriAppVersion.value = null;
  engineVersion.value = null;
  latestEngineVersion.value = null;
  engineRestarting.value = false;
  engineRestartNewVersion.value = false;
  releaseNoticeDismissed.value = false;
  toasts.value = [];
  localStorage.removeItem(RESTART_LS_KEY);
  localStorage.removeItem('lucidos-restart-groups');
  delete (window as any).__LUCIDOS_APP_VERSION__;
});

describe('restart state survives network reconnect', () => {
  function loadedHealth(overrides: Record<string, unknown> = {}) {
    return {
      status: 'loaded',
      data: { workspace: 'test', workspace_path: '/tmp', started_at: STARTED_AT, ...overrides },
    };
  }
  const unreachable = { status: 'failed', error: 'Failed to fetch' };

  async function establishConnection() {
    mockCheckHealth.mockResolvedValueOnce(loadedHealth());
    await checkConnection();
  }

  async function forceDisconnect() {
    for (let i = 0; i < 4; i++) {
      mockCheckHealth.mockResolvedValueOnce(unreachable);
      await checkConnection();
    }
  }

  it('preserves restart state on simple reconnect (same engine)', async () => {
    await establishConnection();

    restartRequired.value = true;
    syncRestartState();
    localStorage.setItem(RESTART_LS_KEY, 'true');

    await forceDisconnect();
    expect(connectionStatus.value).toBe('disconnected');

    mockCheckHealth.mockResolvedValueOnce(loadedHealth());
    await checkConnection();

    expect(restartRequired.value).toBe(true);
    expect(localStorage.getItem(RESTART_LS_KEY)).toBe('true');
  });

  it('re-reads the release notices when the engine restarted under a live page', async () => {
    await establishConnection();
    vi.mocked(loadReleaseNotices).mockClear();

    mockCheckHealth.mockResolvedValueOnce(loadedHealth({ started_at: RESTARTED_AT }));
    await checkConnection();

    expect(loadReleaseNotices).toHaveBeenCalled();
  });

  it('lets a notice from the new build raise itself after an earlier dismissal', async () => {
    await establishConnection();
    releaseNoticeDismissed.value = true;

    mockCheckHealth.mockResolvedValueOnce(loadedHealth({ started_at: RESTARTED_AT }));
    await checkConnection();

    expect(releaseNoticeDismissed.value).toBe(false);
  });

  it('keeps the dismissal across a plain reconnect', async () => {
    await establishConnection();
    releaseNoticeDismissed.value = true;

    await forceDisconnect();
    mockCheckHealth.mockResolvedValueOnce(loadedHealth());
    await checkConnection();

    expect(releaseNoticeDismissed.value).toBe(true);
  });

  it('clears restart state when engine actually restarted (started_at changed)', async () => {
    await establishConnection();

    restartRequired.value = true;
    syncRestartState();
    localStorage.setItem(RESTART_LS_KEY, 'true');

    mockCheckHealth.mockResolvedValueOnce(loadedHealth({ started_at: RESTARTED_AT }));
    await checkConnection();

    expect(restartRequired.value).toBe(false);
    expect(toasts.value.find(t => t.key === RESTART_FAILURE_TOAST_KEY)).toBeFalsy();
    expect(localStorage.getItem(RESTART_LS_KEY)).toBeNull();

    const restartedToast = toasts.value.find(t => t.message === 'Engine restarted');
    expect(restartedToast).toBeTruthy();
    expect(restartedToast!.action).toBeUndefined();
  });

  it('clears restart state on reconnect when engine restarted during disconnect', async () => {
    await establishConnection();

    restartRequired.value = true;
    syncRestartState();
    localStorage.setItem(RESTART_LS_KEY, 'true');

    await forceDisconnect();

    mockCheckHealth.mockResolvedValueOnce(loadedHealth({ started_at: RESTARTED_AT }));
    await checkConnection();

    expect(restartRequired.value).toBe(false);
    expect(toasts.value.find(t => t.key === RESTART_FAILURE_TOAST_KEY)).toBeFalsy();
    expect(localStorage.getItem(RESTART_LS_KEY)).toBeNull();
  });

  it('keeps the in-flight restart dialog up (unchanged) when the old engine goes unreachable', async () => {
    await establishConnection();

    engineRestarting.value = true;
    engineRestartNewVersion.value = true;
    toasts.value = [];

    mockCheckHealth.mockResolvedValueOnce(unreachable);
    await checkConnection();

    expect(engineRestarting.value).toBe(true);
    expect(activeProgressDialog.value.visible).toBe(true);
    expect(activeProgressDialog.value.title).toBe('Starting new version');
  });

  it('does not invent a restart dialog on a transient blip when none is in flight', async () => {
    await establishConnection();

    engineRestarting.value = false;
    toasts.value = [];

    mockCheckHealth.mockResolvedValueOnce(unreachable);
    await checkConnection();

    expect(activeProgressDialog.value.visible).toBe(false);
    expect(toasts.value.find(t => t.key === RESTART_FAILURE_TOAST_KEY)).toBeUndefined();
  });

  it('does not set updateAvailable on engine restart when the frontend bundle is unchanged', async () => {
    await establishConnection();
    updateAvailable.value = false;

    mockCheckHealth.mockResolvedValueOnce(loadedHealth({ started_at: RESTARTED_AT }));
    await checkConnection();

    expect(updateAvailable.value).toBe(false);
  });

  it('does not set updateAvailable on regular health check (same engine)', async () => {
    await establishConnection();
    updateAvailable.value = false;

    mockCheckHealth.mockResolvedValueOnce(loadedHealth());
    await checkConnection();

    expect(updateAvailable.value).toBe(false);
  });

  it('sets updateAvailable when Tauri app version is outdated', async () => {
    await establishConnection();
    updateAvailable.value = false;
    (window as any).__LUCIDOS_APP_VERSION__ = '2026.03.01.0';

    mockCheckHealth.mockResolvedValueOnce(loadedHealth({ latest_tauri_app_version: '2026.04.01.0' }));
    await checkConnection();

    expect(updateAvailable.value).toBe(true);
  });

  it('does not set updateAvailable when Tauri app version is current', async () => {
    await establishConnection();
    updateAvailable.value = false;
    (window as any).__LUCIDOS_APP_VERSION__ = '2026.04.01.0';

    mockCheckHealth.mockResolvedValueOnce(loadedHealth({ latest_tauri_app_version: '2026.04.01.0' }));
    await checkConnection();

    expect(updateAvailable.value).toBe(false);
  });

  it('does not set updateAvailable for Tauri version when not running in Tauri', async () => {
    await establishConnection();
    updateAvailable.value = false;

    mockCheckHealth.mockResolvedValueOnce(loadedHealth({ latest_tauri_app_version: '2026.04.01.0' }));
    await checkConnection();

    expect(updateAvailable.value).toBe(false);
  });

  it('does not set updateAvailable when only the engine version is ahead (engine-only bump)', async () => {
    await establishConnection();
    updateAvailable.value = false;
    mockCheckHealth.mockResolvedValueOnce(loadedHealth({
      engine_version: '2026.06.07.1',
      latest_engine_version: '2026.06.07.1',
    }));
    await checkConnection();

    expect(updateAvailable.value).toBe(false);
    expect(restartRequired.value).toBe(false);
  });

  it('sets restartRequired when latest_engine_version is newer than engine_version', async () => {
    await establishConnection();
    restartRequired.value = false;

    mockCheckHealth.mockResolvedValueOnce(loadedHealth({
      engine_version: '2026.04.01.1',
      latest_engine_version: '2026.04.13.1',
    }));
    await checkConnection();

    expect(restartRequired.value).toBe(true);
    expect(engineVersion.value).toBe('2026.04.01.1');
    expect(latestEngineVersion.value).toBe('2026.04.13.1');
  });

  it('a newer engine asks for a restart but never raises the web update prompt', async () => {
    await establishConnection();
    restartRequired.value = false;
    updateAvailable.value = false;

    mockCheckHealth.mockResolvedValueOnce(loadedHealth({
      engine_version: '2026.04.01.1',
      latest_engine_version: '2026.04.13.1',
    }));
    await checkConnection();

    expect(restartRequired.value).toBe(true);
    expect(updateAvailable.value).toBe(false);
  });

  it('does not set restartRequired when engine version is current', async () => {
    await establishConnection();
    restartRequired.value = false;

    mockCheckHealth.mockResolvedValueOnce(loadedHealth({
      engine_version: '2026.04.13.1',
      latest_engine_version: '2026.04.13.1',
    }));
    await checkConnection();

    expect(restartRequired.value).toBe(false);
  });

  it('does not set restartRequired when latest_engine_version is missing', async () => {
    await establishConnection();
    restartRequired.value = false;

    mockCheckHealth.mockResolvedValueOnce(loadedHealth({
      engine_version: '2026.04.01.1',
    }));
    await checkConnection();

    expect(restartRequired.value).toBe(false);
  });

  describe('handleRestartTimeout (restart safety timeout)', () => {
    it('invalidates status when a timed-out restart leaves the same engine reachable', async () => {
      await establishConnection();
      engineRestarting.value = true;
      const before = engineVersionStatusVersion.peek();
      mockCheckHealth.mockResolvedValue(loadedHealth({ started_at: STARTED_AT }));
      await handleRestartTimeout();
      expect(engineRestarting.value).toBe(false);
      expect(engineVersionStatusVersion.peek()).toBeGreaterThan(before);
    });

    it('does NOT show a timeout error when the engine is reachable (frozen iOS timer fired on resume)', async () => {
      await establishConnection();
      engineRestarting.value = true;
      toasts.value = [];

      mockCheckHealth.mockResolvedValue(loadedHealth({ started_at: RESTARTED_AT }));
      await handleRestartTimeout();

      expect(toasts.value.find(t => t.message === 'Engine restart timed out')).toBeUndefined();
      expect(engineRestarting.value).toBe(false);
      expect(toasts.value.find(t => t.message === 'Engine restarted')).toBeTruthy();
    });

    it('shows the timeout error and unblocks the UI when the engine is genuinely unreachable', async () => {
      await establishConnection();
      engineRestarting.value = true;
      toasts.value = [];

      mockCheckHealth.mockResolvedValueOnce(unreachable);
      await handleRestartTimeout();

      expect(engineRestarting.value).toBe(false);
      expect(activeProgressDialog.value.visible).toBe(false);
      const toast = toasts.value.find(t => t.message === 'Engine restart timed out');
      expect(toast).toBeTruthy();
      expect(toast!.type).toBe('error');
    });
  });
});
