import { describe, it, expect, beforeEach, vi } from 'vitest';
import { connectionStatus, toasts, restartRequired, restartGroups, engineStartedAt, engineRestarting, updateAvailable } from '../store';
import { restoreRestartToast, syncRestartToast, RESTART_LS_KEY, RESTART_IN_FLIGHT_LS_KEY, RESTART_GROUPS_LS_KEY } from '../actions/chat-changes';

// Mock dependencies so checkConnection runs in isolation. Mirrors
// restart-toast-reconnect.test.ts, but this file deliberately NEVER calls an
// establishConnection() helper: connection.ts's module-private `hasEverConnected`
// stays false the whole time (a genuine fresh page load), so the completion test
// truly exercises the `everOrRestarting` fallback that restored `engineRestarting`
// unlocks — rather than being masked by a prior connect having set the flag.
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
  refreshThreadEvents: vi.fn().mockResolvedValue(true),
  // runResumeSync retries every thread carrying `eventsLoadFailed` through
  // this, so the reference is read on every resume even when nothing is
  // flagged; the mock proxy throws on an undeclared export.
  loadThreadEvents: vi.fn().mockResolvedValue(undefined),
  clearThreadFetchGuards: vi.fn(),
  markLoadedThreadsStale: vi.fn(),
}));
vi.mock('../actions/chat-changes', async () => {
  const actual = await vi.importActual('../actions/chat-changes') as any;
  return { ...actual, refreshChangesState: vi.fn() };
});
vi.mock('../actions/notifications', () => ({
  loadUnreadNotifications: vi.fn(),
}));

const { checkConnection } = await import('../actions/connection');

const RESTART_TOAST_KEY = 'restart-required';
const STARTED_AT = '2026-06-28T06:00:00Z';
const RESTARTED_AT = '2026-06-28T07:00:00Z';
const NEW_VERSION_MESSAGE = 'Starting new version…';
const PLAIN_MESSAGE = 'Restarting engine…';

function loadedHealth(overrides: Record<string, unknown> = {}) {
  return {
    status: 'loaded',
    data: { workspace: 'test', workspace_path: '/tmp', started_at: STARTED_AT, ...overrides },
  };
}
const unreachable = { status: 'failed', error: 'Failed to fetch' };

/** Seed localStorage as if a restart was in flight when the page unloaded, then
 *  reset the signals to fresh-reload defaults and run the startup restore.
 *  `newVersion` mirrors what initiateEngineRestart persisted — whether the restart
 *  delivers a new engine version (→ "Starting new version…") or is a plain respawn
 *  (→ "Restarting engine…"). */
function reloadMidRestart(newVersion: boolean): void {
  // syncRestartToast set RESTART_LS_KEY; initiateEngineRestart set the in-flight marker.
  localStorage.setItem(RESTART_LS_KEY, 'true');
  localStorage.setItem(RESTART_IN_FLIGHT_LS_KEY, JSON.stringify({ startedAt: STARTED_AT, newVersion }));
  connectionStatus.value = 'connecting';
  engineStartedAt.value = null;
  engineRestarting.value = false;
  restartRequired.value = false;
  toasts.value = [];
  restoreRestartToast();
}

beforeEach(() => {
  vi.clearAllMocks();
  connectionStatus.value = 'connecting';
  engineStartedAt.value = null;
  engineRestarting.value = false;
  restartRequired.value = false;
  restartGroups.value = [];
  updateAvailable.value = false;
  toasts.value = [];
  localStorage.removeItem(RESTART_LS_KEY);
  localStorage.removeItem(RESTART_IN_FLIGHT_LS_KEY);
  localStorage.removeItem(RESTART_GROUPS_LS_KEY);
});

describe('restart progress toast survives a reload (in-flight marker)', () => {
  it('restores the PROGRESS toast (not the warning) and re-arms restart state', () => {
    reloadMidRestart(true);

    const toast = toasts.value.find(t => t.key === RESTART_TOAST_KEY);
    expect(toast).toBeTruthy();
    expect(toast!.message).toBe(NEW_VERSION_MESSAGE);
    expect(toast!.spinning).toBe(true);
    // NOT the pre-restart "Engine restart required" warning — that one carries a
    // Restart action; the progress toast has none.
    expect(toast!.action).toBeUndefined();
    expect(toast!.type).toBe('info');

    // State re-armed so checkConnection can detect the completion across the reload.
    expect(engineRestarting.value).toBe(true);
    expect(engineStartedAt.value).toBe(STARTED_AT);
    expect(restartRequired.value).toBe(true);
  });

  it('restores the plain "Restarting engine…" wording when no new version is delivered', () => {
    reloadMidRestart(false);

    expect(toasts.value.find(t => t.key === RESTART_TOAST_KEY)!.message).toBe(PLAIN_MESSAGE);
  });

  it('carries the restored toast to "Engine restarted" on reconnect with a new started_at', async () => {
    reloadMidRestart(true);
    expect(engineRestarting.value).toBe(true);

    // Engine comes back with a NEW started_at. hasEverConnected is false (fresh
    // load); the restored engineRestarting unlocks the completion detection.
    mockCheckHealth.mockResolvedValueOnce(loadedHealth({ started_at: RESTARTED_AT }));
    await checkConnection();

    expect(engineRestarting.value).toBe(false);
    expect(toasts.value.find(t => t.key === RESTART_TOAST_KEY)).toBeFalsy();
    expect(toasts.value.find(t => t.message === 'Engine restarted')).toBeTruthy();
    // Both markers cleared so a later reload won't restore a stale progress toast.
    expect(localStorage.getItem(RESTART_LS_KEY)).toBeNull();
    expect(localStorage.getItem(RESTART_IN_FLIGHT_LS_KEY)).toBeNull();
  });

  it('keeps the restored progress toast up (unchanged) when the old engine goes unreachable', async () => {
    reloadMidRestart(true);
    expect(toasts.value.find(t => t.key === RESTART_TOAST_KEY)!.message).toBe(NEW_VERSION_MESSAGE);

    // Old engine killed → a /health failure must NOT dismiss or reword the toast
    // (there is no build→swap phase transition); it stays with its spinner.
    mockCheckHealth.mockResolvedValueOnce(unreachable);
    await checkConnection();

    const toast = toasts.value.find(t => t.key === RESTART_TOAST_KEY);
    expect(toast!.message).toBe(NEW_VERSION_MESSAGE);
    expect(toast!.spinning).toBe(true);
    // Still restarting — the flag must NOT clear on a mere disconnect.
    expect(engineRestarting.value).toBe(true);
  });

  it('does not hang: even a stale marker self-heals on the next connect', async () => {
    // The clear was somehow missed: the restart already finished, but the marker
    // survived. Restore shows the toast, then the first poll sees a different
    // started_at and completes — engineRestarting can never hang.
    reloadMidRestart(false);

    mockCheckHealth.mockResolvedValueOnce(loadedHealth({ started_at: RESTARTED_AT }));
    await checkConnection();

    expect(engineRestarting.value).toBe(false);
    expect(localStorage.getItem(RESTART_IN_FLIGHT_LS_KEY)).toBeNull();
  });

  it('a re-sync that flips restartRequired false does NOT dismiss the restored progress toast', () => {
    // After restore, the startup refreshChangesState resolves with the applied
    // change no longer pending (restart_required=false) and calls syncRestartToast.
    // While engineRestarting is true, that must leave the progress toast alone —
    // otherwise the user reloaded mid-restart and the toast vanishes.
    reloadMidRestart(true);
    expect(toasts.value.find(t => t.key === RESTART_TOAST_KEY)!.message).toBe(NEW_VERSION_MESSAGE);

    restartRequired.value = false; // backend: applied change dropped out of pending
    syncRestartToast();

    const toast = toasts.value.find(t => t.key === RESTART_TOAST_KEY);
    expect(toast).toBeTruthy();
    expect(toast!.message).toBe(NEW_VERSION_MESSAGE);
    expect(engineRestarting.value).toBe(true);
  });

  it('cold start with only a PENDING restart (no in-flight marker) restores state, not a toast', () => {
    // No in-flight marker — a restart is merely pending, not underway. There is no
    // pre-switch toast anymore; restore re-arms restartRequired so the
    // brand badge + restart confirm dialog reappear. The engine "New
    // version available → Switch" toast is owned by the poll (engine-update.ts).
    localStorage.setItem(RESTART_LS_KEY, 'true');
    engineRestarting.value = false;
    toasts.value = [];

    restoreRestartToast();

    expect(restartRequired.value).toBe(true);
    expect(toasts.value.find(t => t.key === RESTART_TOAST_KEY)).toBeFalsy();
    expect(engineRestarting.value).toBe(false);
  });
});
