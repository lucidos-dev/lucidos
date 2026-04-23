import { describe, it, expect, beforeEach, vi } from 'vitest';
import { connectionStatus, toasts, restartRequired, restartGroups, engineStartedAt, updateAvailable, latestTauriAppVersion, engineVersion, latestEngineVersion } from '../store';
import { syncRestartToast, RESTART_LS_KEY } from '../actions/chat-changes';

// Mock dependencies so checkConnection can run in isolation
const mockCheckHealth = vi.fn();
vi.mock('../../api/client', () => ({
  checkHealth: (...args: any[]) => mockCheckHealth(...args),
  API_BASE: 'http://localhost:3000',
}));
vi.mock('../actions/thread-sync', () => ({
  connectThreadEvents: vi.fn(),
  disconnectThreadEvents: vi.fn(),
}));
vi.mock('../actions/thread-loading', () => ({
  loadAllThreads: vi.fn().mockResolvedValue(undefined),
  refreshThreadEvents: vi.fn().mockResolvedValue(undefined),
  clearForcedRetries: vi.fn(),
}));
vi.mock('../actions/chat-changes', async () => {
  const actual = await vi.importActual('../actions/chat-changes') as any;
  return {
    ...actual,
    refreshChangesState: vi.fn(),
  };
});
vi.mock('../actions/notifications', () => ({
  refreshUnreadCount: vi.fn(),
}));

const { checkConnection } = await import('../actions/connection');

const RESTART_TOAST_KEY = 'restart-required';
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
  toasts.value = [];
  localStorage.removeItem(RESTART_LS_KEY);
  localStorage.removeItem('cognos-restart-groups');
  delete (window as any).__COGNOS_APP_VERSION__;
});

describe('restart toast survives network reconnect', () => {
  function health(overrides: Record<string, unknown> = {}) {
    return { workspace: 'test', workspace_path: '/tmp', started_at: STARTED_AT, ...overrides };
  }

  /** Helper: simulate initial connection to set hasEverConnected=true */
  async function establishConnection() {
    mockCheckHealth.mockResolvedValueOnce(health());
    await checkConnection();
    // Now hasEverConnected=true, engineStartedAt=STARTED_AT
  }

  it('preserves restart toast on simple reconnect (same engine)', async () => {
    await establishConnection();

    // Show restart toast (simulates ChangesUpdated SSE with restart_required=true)
    restartRequired.value = true;
    syncRestartToast();
    localStorage.setItem(RESTART_LS_KEY, 'true');
    expect(toasts.value.find(t => t.key === RESTART_TOAST_KEY)).toBeTruthy();

    // Engine becomes briefly unreachable (health check timeout during apply)
    mockCheckHealth.mockResolvedValueOnce(null);
    await checkConnection();
    expect(connectionStatus.value).toBe('disconnected');

    // Engine comes back — same started_at (NOT a restart, just network hiccup)
    mockCheckHealth.mockResolvedValueOnce(health());
    await checkConnection();

    // Toast must survive — the engine didn't restart
    expect(toasts.value.find(t => t.key === RESTART_TOAST_KEY)).toBeTruthy();
    expect(localStorage.getItem(RESTART_LS_KEY)).toBe('true');
  });

  it('dismisses restart toast when engine actually restarted (started_at changed)', async () => {
    await establishConnection();

    // Show restart toast
    restartRequired.value = true;
    syncRestartToast();
    localStorage.setItem(RESTART_LS_KEY, 'true');
    expect(toasts.value.find(t => t.key === RESTART_TOAST_KEY)).toBeTruthy();

    // Engine restarts — started_at changes
    mockCheckHealth.mockResolvedValueOnce(health({ started_at: RESTARTED_AT }));
    await checkConnection();

    // Toast must be dismissed — engine restarted
    expect(toasts.value.find(t => t.key === RESTART_TOAST_KEY)).toBeFalsy();
    expect(localStorage.getItem(RESTART_LS_KEY)).toBeNull();
  });

  it('dismisses restart toast on reconnect when engine restarted during disconnect', async () => {
    await establishConnection();

    // Show restart toast
    restartRequired.value = true;
    syncRestartToast();
    localStorage.setItem(RESTART_LS_KEY, 'true');

    // Engine goes down
    mockCheckHealth.mockResolvedValueOnce(null);
    await checkConnection();

    // Engine comes back with NEW started_at (restarted while we were disconnected)
    mockCheckHealth.mockResolvedValueOnce(health({ started_at: RESTARTED_AT }));
    await checkConnection();

    // Toast must be dismissed
    expect(toasts.value.find(t => t.key === RESTART_TOAST_KEY)).toBeFalsy();
    expect(localStorage.getItem(RESTART_LS_KEY)).toBeNull();
  });

  it('sets updateAvailable when engine restarts', async () => {
    await establishConnection();
    // establishConnection may set updateAvailable due to persisted module state — reset
    updateAvailable.value = false;

    // Engine restarts with new started_at
    mockCheckHealth.mockResolvedValueOnce(health({ started_at: RESTARTED_AT }));
    await checkConnection();

    expect(updateAvailable.value).toBe(true);
  });

  it('does not set updateAvailable on regular health check (same engine)', async () => {
    await establishConnection();
    updateAvailable.value = false;

    // Regular health check with same started_at — not a restart
    mockCheckHealth.mockResolvedValueOnce(health());
    await checkConnection();

    expect(updateAvailable.value).toBe(false);
  });

  it('sets updateAvailable when Tauri app version is outdated', async () => {
    await establishConnection();
    updateAvailable.value = false;
    (window as any).__COGNOS_APP_VERSION__ = '2026.03.01.0';

    mockCheckHealth.mockResolvedValueOnce(health({ latest_tauri_app_version: '2026.04.01.0' }));
    await checkConnection();

    expect(updateAvailable.value).toBe(true);
  });

  it('does not set updateAvailable when Tauri app version is current', async () => {
    await establishConnection();
    updateAvailable.value = false;
    (window as any).__COGNOS_APP_VERSION__ = '2026.04.01.0';

    mockCheckHealth.mockResolvedValueOnce(health({ latest_tauri_app_version: '2026.04.01.0' }));
    await checkConnection();

    expect(updateAvailable.value).toBe(false);
  });

  it('does not set updateAvailable for Tauri version when not running in Tauri', async () => {
    await establishConnection();
    updateAvailable.value = false;
    // window.__COGNOS_APP_VERSION__ is undefined (browser mode)

    mockCheckHealth.mockResolvedValueOnce(health({ latest_tauri_app_version: '2026.04.01.0' }));
    await checkConnection();

    expect(updateAvailable.value).toBe(false);
  });

  it('sets restartRequired when latest_engine_version is newer than engine_version', async () => {
    await establishConnection();
    restartRequired.value = false;

    mockCheckHealth.mockResolvedValueOnce(health({
      engine_version: '2026.04.01.1',
      latest_engine_version: '2026.04.13.1',
    }));
    await checkConnection();

    expect(restartRequired.value).toBe(true);
    expect(engineVersion.value).toBe('2026.04.01.1');
    expect(latestEngineVersion.value).toBe('2026.04.13.1');
  });

  it('does not set restartRequired when engine version is current', async () => {
    await establishConnection();
    restartRequired.value = false;

    mockCheckHealth.mockResolvedValueOnce(health({
      engine_version: '2026.04.13.1',
      latest_engine_version: '2026.04.13.1',
    }));
    await checkConnection();

    expect(restartRequired.value).toBe(false);
  });

  it('does not set restartRequired when latest_engine_version is missing', async () => {
    await establishConnection();
    restartRequired.value = false;

    mockCheckHealth.mockResolvedValueOnce(health({
      engine_version: '2026.04.01.1',
    }));
    await checkConnection();

    expect(restartRequired.value).toBe(false);
  });
});
