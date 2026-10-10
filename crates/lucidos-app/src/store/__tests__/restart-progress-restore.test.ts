import { describe, it, expect, beforeEach, vi } from 'vitest';
import { connectionStatus, toasts, restartRequired, restartGroups, engineStartedAt, engineRestarting, engineRestartNewVersion, activeProgressDialog, updateAvailable } from '../store';
import { restoreRestartState, syncRestartState, RESTART_LS_KEY, RESTART_IN_FLIGHT_LS_KEY, RESTART_GROUPS_LS_KEY } from '../actions/chat-changes';

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

const STARTED_AT = '2026-06-28T06:00:00Z';
const RESTARTED_AT = '2026-06-28T07:00:00Z';
const NEW_VERSION_TITLE = 'Starting new version';
const PLAIN_TITLE = 'Restarting engine';

function loadedHealth(overrides: Record<string, unknown> = {}) {
  return {
    status: 'loaded',
    data: { workspace: 'test', workspace_path: '/tmp', started_at: STARTED_AT, ...overrides },
  };
}
const unreachable = { status: 'failed', error: 'Failed to fetch' };

/** Seed localStorage as if a restart was in flight when the page unloaded, then
 *  reset the signals to fresh-reload defaults and run the startup restore.
 *  `newVersion` mirrors what initiateEngineRestart persisted: whether the
 *  restart delivers a new engine version (→ "Starting new version") or is a
 *  plain respawn (→ "Restarting engine"). */
function reloadMidRestart(newVersion: boolean): void {
  // syncRestartState set RESTART_LS_KEY; initiateEngineRestart set the in-flight marker.
  localStorage.setItem(RESTART_LS_KEY, 'true');
  localStorage.setItem(RESTART_IN_FLIGHT_LS_KEY, JSON.stringify({ startedAt: STARTED_AT, newVersion }));
  connectionStatus.value = 'connecting';
  engineStartedAt.value = null;
  engineRestarting.value = false;
  engineRestartNewVersion.value = false;
  restartRequired.value = false;
  toasts.value = [];
  restoreRestartState();
}

beforeEach(() => {
  vi.clearAllMocks();
  connectionStatus.value = 'connecting';
  engineStartedAt.value = null;
  engineRestarting.value = false;
  engineRestartNewVersion.value = false;
  restartRequired.value = false;
  restartGroups.value = [];
  updateAvailable.value = false;
  toasts.value = [];
  localStorage.removeItem(RESTART_LS_KEY);
  localStorage.removeItem(RESTART_IN_FLIGHT_LS_KEY);
  localStorage.removeItem(RESTART_GROUPS_LS_KEY);
});

describe('restart progress dialog survives a reload (in-flight marker)', () => {
  it('restores the PROGRESS DIALOG (not the warning) and re-arms restart state', () => {
    reloadMidRestart(true);

    const dialog = activeProgressDialog.value;
    expect(dialog.visible).toBe(true);
    expect(dialog.title).toBe(NEW_VERSION_TITLE);
    // A restart cannot be called back, so the dialog offers no way out of it.
    expect(dialog.cancel).toBeUndefined();
    // No toast either: the pre-restart "Engine restart required" warning would
    // nag the user to start a restart already underway.
    expect(toasts.value).toHaveLength(0);

    // State re-armed so checkConnection can detect the completion across the reload.
    expect(engineRestarting.value).toBe(true);
    expect(engineStartedAt.value).toBe(STARTED_AT);
    expect(restartRequired.value).toBe(true);
  });

  it('restores the plain "Restarting engine" title when no new version is delivered', () => {
    reloadMidRestart(false);

    expect(activeProgressDialog.value.title).toBe(PLAIN_TITLE);
  });

  it('carries the restored dialog to "Engine restarted" on reconnect with a new started_at', async () => {
    reloadMidRestart(true);
    expect(engineRestarting.value).toBe(true);

    // Engine comes back with a NEW started_at. hasEverConnected is false (fresh
    // load); the restored engineRestarting unlocks the completion detection.
    mockCheckHealth.mockResolvedValueOnce(loadedHealth({ started_at: RESTARTED_AT }));
    await checkConnection();

    expect(engineRestarting.value).toBe(false);
    expect(activeProgressDialog.value.visible).toBe(false);
    expect(toasts.value.find(t => t.message === 'Engine restarted')).toBeTruthy();
    // Both markers cleared so a later reload won't restore a stale dialog.
    expect(localStorage.getItem(RESTART_LS_KEY)).toBeNull();
    expect(localStorage.getItem(RESTART_IN_FLIGHT_LS_KEY)).toBeNull();
  });

  it('keeps the restored dialog up (unchanged) when the old engine goes unreachable', async () => {
    reloadMidRestart(true);
    expect(activeProgressDialog.value.title).toBe(NEW_VERSION_TITLE);

    // Old engine killed. A /health failure must NOT close or reword the dialog,
    // since there is no build to swap phase transition.
    mockCheckHealth.mockResolvedValueOnce(unreachable);
    await checkConnection();

    expect(activeProgressDialog.value.visible).toBe(true);
    expect(activeProgressDialog.value.title).toBe(NEW_VERSION_TITLE);
    // Still restarting: the flag must NOT clear on a mere disconnect.
    expect(engineRestarting.value).toBe(true);
  });

  it('does not hang: even a stale marker self-heals on the next connect', async () => {
    // The clear was somehow missed: the restart already finished, but the marker
    // survived. Restore raises the dialog, then the first poll sees a different
    // started_at and completes. engineRestarting can never hang.
    reloadMidRestart(false);

    mockCheckHealth.mockResolvedValueOnce(loadedHealth({ started_at: RESTARTED_AT }));
    await checkConnection();

    expect(engineRestarting.value).toBe(false);
    expect(localStorage.getItem(RESTART_IN_FLIGHT_LS_KEY)).toBeNull();
  });

  it('a re-sync that flips restartRequired false does NOT close the restored dialog', () => {
    // After restore, the startup refreshChangesState resolves with the applied
    // change no longer pending (restart_required=false) and calls
    // syncRestartState. While engineRestarting is true that must leave the
    // dialog alone, or a user who reloaded mid-restart watches it vanish.
    reloadMidRestart(true);
    expect(activeProgressDialog.value.title).toBe(NEW_VERSION_TITLE);

    restartRequired.value = false; // backend: applied change dropped out of pending
    syncRestartState();

    expect(activeProgressDialog.value.visible).toBe(true);
    expect(activeProgressDialog.value.title).toBe(NEW_VERSION_TITLE);
    expect(engineRestarting.value).toBe(true);
  });

  it('cold start with only a PENDING restart (no in-flight marker) surfaces nothing', () => {
    // No in-flight marker: a restart is merely pending, not underway. Restore
    // re-arms restartRequired so the brand badge and the restart confirm dialog
    // reappear. The engine "New version available → Switch" toast is owned by
    // the poll (engine-update.ts).
    localStorage.setItem(RESTART_LS_KEY, 'true');
    engineRestarting.value = false;
    toasts.value = [];

    restoreRestartState();

    expect(restartRequired.value).toBe(true);
    expect(activeProgressDialog.value.visible).toBe(false);
    expect(toasts.value).toHaveLength(0);
    expect(engineRestarting.value).toBe(false);
  });
});
