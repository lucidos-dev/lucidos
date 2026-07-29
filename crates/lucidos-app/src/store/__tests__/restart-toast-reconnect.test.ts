import { describe, it, expect, beforeEach, vi } from 'vitest';
import { connectionStatus, toasts, showToast, restartRequired, restartGroups, engineStartedAt, updateAvailable, latestTauriAppVersion, engineVersion, latestEngineVersion, engineRestarting } from '../store';
import { syncRestartToast, RESTART_LS_KEY } from '../actions/chat-changes';

// Mock dependencies so checkConnection can run in isolation
const mockCheckHealth = vi.fn();
vi.mock('../../api/client', () => ({
  checkHealth: (...args: any[]) => mockCheckHealth(...args),
  API_BASE: 'http://localhost:3000',
  API: 'http://localhost:3000/api/v1',
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
  loadUnreadNotifications: vi.fn(),
}));

const { checkConnection, handleRestartTimeout } = await import('../actions/connection');

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
  engineRestarting.value = false;
  toasts.value = [];
  localStorage.removeItem(RESTART_LS_KEY);
  localStorage.removeItem('lucidos-restart-groups');
  delete (window as any).__LUCIDOS_APP_VERSION__;
});

describe('restart toast survives network reconnect', () => {
  function loadedHealth(overrides: Record<string, unknown> = {}) {
    return {
      status: 'loaded',
      data: { workspace: 'test', workspace_path: '/tmp', started_at: STARTED_AT, ...overrides },
    };
  }
  const unreachable = { status: 'failed', error: 'Failed to fetch' };

  /** Helper: simulate initial connection to set hasEverConnected=true */
  async function establishConnection() {
    mockCheckHealth.mockResolvedValueOnce(loadedHealth());
    await checkConnection();
    // Now hasEverConnected=true, engineStartedAt=STARTED_AT
  }

  /** Drive a genuine disconnect. The dot debounces transient /health failures
   *  (MAX_SUPPRESSED_FAILURES in connection.ts), so it only flips to
   *  'disconnected' after several consecutive failures — a single blip stays
   *  green. Fail enough times to exceed the debounce window. */
  async function forceDisconnect() {
    for (let i = 0; i < 4; i++) {
      mockCheckHealth.mockResolvedValueOnce(unreachable);
      await checkConnection();
    }
  }

  it('preserves restart state on simple reconnect (same engine)', async () => {
    await establishConnection();

    // Seed pending restart state (simulates ChangeApplied with requires_restart=true)
    restartRequired.value = true;
    syncRestartToast();
    localStorage.setItem(RESTART_LS_KEY, 'true');

    // Engine becomes unreachable (sustained health-check timeouts during apply)
    await forceDisconnect();
    expect(connectionStatus.value).toBe('disconnected');

    // Engine comes back — same started_at (NOT a restart, just network hiccup)
    mockCheckHealth.mockResolvedValueOnce(loadedHealth());
    await checkConnection();

    // Restart state must survive — the engine didn't restart (no toast; the state
    // drives the control-panel badge + confirm dialog).
    expect(restartRequired.value).toBe(true);
    expect(localStorage.getItem(RESTART_LS_KEY)).toBe('true');
  });

  it('clears restart state when engine actually restarted (started_at changed)', async () => {
    await establishConnection();

    // Seed pending restart state
    restartRequired.value = true;
    syncRestartToast();
    localStorage.setItem(RESTART_LS_KEY, 'true');

    // Engine restarts — started_at changes
    mockCheckHealth.mockResolvedValueOnce(loadedHealth({ started_at: RESTARTED_AT }));
    await checkConnection();

    // The switch completed — the engine-pending gate is cleared so the deferred
    // client refresh toast can surface, and any keyed restart toast is dismissed.
    expect(restartRequired.value).toBe(false);
    expect(toasts.value.find(t => t.key === RESTART_TOAST_KEY)).toBeFalsy();
    expect(localStorage.getItem(RESTART_LS_KEY)).toBeNull();

    // The post-restart confirmation is a plain, action-LESS success toast — no
    // Refresh nag, because an engine-only restart leaves the client in sync. The
    // refresh prompt is owned solely by the build-id staleness check.
    const restartedToast = toasts.value.find(t => t.message === 'Engine restarted');
    expect(restartedToast).toBeTruthy();
    expect(restartedToast!.action).toBeUndefined();
  });

  it('clears restart state on reconnect when engine restarted during disconnect', async () => {
    await establishConnection();

    // Seed pending restart state
    restartRequired.value = true;
    syncRestartToast();
    localStorage.setItem(RESTART_LS_KEY, 'true');

    // Engine goes down (sustained failures past the debounce window)
    await forceDisconnect();

    // Engine comes back with NEW started_at (restarted while we were disconnected)
    mockCheckHealth.mockResolvedValueOnce(loadedHealth({ started_at: RESTARTED_AT }));
    await checkConnection();

    // Switch completed — gate cleared, keyed toast dismissed
    expect(restartRequired.value).toBe(false);
    expect(toasts.value.find(t => t.key === RESTART_TOAST_KEY)).toBeFalsy();
    expect(localStorage.getItem(RESTART_LS_KEY)).toBeNull();
  });

  it('keeps the in-flight restart status toast up (unchanged) when the old engine goes unreachable', async () => {
    await establishConnection();

    // Restart in flight: the status toast is up (initiateEngineRestart). Its wording
    // is stable for the whole window — there is no build→swap phase transition.
    engineRestarting.value = true;
    toasts.value = [];
    showToast('Starting new version…', 'info', { key: RESTART_TOAST_KEY, showDuringRestart: true, spinning: true });

    // Old engine killed → a /health failure must NOT dismiss or reword the toast;
    // it stays with its spinner until reconnect (started_at change) clears it.
    mockCheckHealth.mockResolvedValueOnce(unreachable);
    await checkConnection();

    const toast = toasts.value.find(t => t.key === RESTART_TOAST_KEY);
    expect(toast).toBeTruthy();
    expect(toast!.message).toBe('Starting new version…');
    expect(toast!.spinning).toBe(true);
  });

  it('does not invent a restart toast on a transient blip when no restart is in flight', async () => {
    await establishConnection();

    // No restart underway — a one-off health failure must not invent a progress toast.
    engineRestarting.value = false;
    toasts.value = [];

    mockCheckHealth.mockResolvedValueOnce(unreachable);
    await checkConnection();

    expect(toasts.value.find(t => t.key === RESTART_TOAST_KEY)).toBeUndefined();
  });

  it('does not set updateAvailable on engine restart when the frontend bundle is unchanged', async () => {
    await establishConnection();
    // establishConnection may set updateAvailable due to persisted module state — reset
    updateAvailable.value = false;

    // Engine restarts (started_at changes) but there's no service worker /
    // frontend rebuild in the test env, so the BUILD_ID staleness check finds
    // nothing newer and the badge stays dark. An engine-only (Rust) restart must
    // NOT nag for a client refresh that does nothing — that was the phantom
    // "Client update available" bug.
    mockCheckHealth.mockResolvedValueOnce(loadedHealth({ started_at: RESTARTED_AT }));
    await checkConnection();

    expect(updateAvailable.value).toBe(false);
  });

  it('does not set updateAvailable on regular health check (same engine)', async () => {
    await establishConnection();
    updateAvailable.value = false;

    // Regular health check with same started_at — not a restart
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
    // window.__LUCIDOS_APP_VERSION__ is undefined (browser mode)

    mockCheckHealth.mockResolvedValueOnce(loadedHealth({ latest_tauri_app_version: '2026.04.01.0' }));
    await checkConnection();

    expect(updateAvailable.value).toBe(false);
  });

  it('does not set updateAvailable when only the engine version is ahead (engine-only bump)', async () => {
    await establishConnection();
    updateAvailable.value = false;
    // Browser mode (no __LUCIDOS_APP_VERSION__). A Rust-only change bumped the
    // engine to .07.1 (engine_version === latest_engine_version → no restart),
    // but the frontend bundle was never rebuilt. The web client must NOT be
    // flagged behind off the engine version — there's no newer frontend build to
    // refresh to. (Frontend freshness is the SW BUILD_ID check, not this.)
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

  // The restart safety timeout (UiBlockingOverlay) probes health before declaring a
  // timeout. On iOS the timer is frozen while the PWA is suspended and fires on
  // resume even though the engine restarted fine while backgrounded — a blind
  // "Engine restart timed out" toast there is a false error (and clearing
  // engineRestarting drops the toast suppression, leaking the resync's transient
  // failures as a pile of toasts). Reported by the user as "These should not
  // appear when coming back to app after restarting!".
  describe('handleRestartTimeout (restart safety timeout)', () => {
    it('does NOT show a timeout error when the engine is reachable (frozen iOS timer fired on resume)', async () => {
      await establishConnection();
      engineRestarting.value = true;
      toasts.value = [];

      // Engine restarted fine while suspended — every health probe succeeds with
      // a NEW started_at (mockResolvedValue, not Once: handleRestartTimeout probes
      // health AND the delegated checkConnection probes again).
      mockCheckHealth.mockResolvedValue(loadedHealth({ started_at: RESTARTED_AT }));
      await handleRestartTimeout();

      expect(toasts.value.find(t => t.message === 'Engine restart timed out')).toBeUndefined();
      // Delegated to checkConnection, which detected the real restart, unblocked
      // the UI, and surfaced the genuine toast instead of a false timeout.
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
      const toast = toasts.value.find(t => t.message === 'Engine restart timed out');
      expect(toast).toBeTruthy();
      expect(toast!.type).toBe('error');
    });
  });
});
