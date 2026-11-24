import { describe, it, expect, beforeEach, vi } from 'vitest';
import { connectionStatus, toasts, restartRequired, restartGroups, engineStartedAt, engineRestarting, engineRestartNewVersion, activeProgressDialog, updateAvailable } from '../store';
import { restoreRestartState, syncRestartState, RESTART_LS_KEY, RESTART_IN_FLIGHT_LS_KEY, RESTART_GROUPS_LS_KEY } from '../actions/chat-changes';

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

function reloadMidRestart(newVersion: boolean): void {
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
    expect(dialog.cancel).toBeUndefined();
    expect(toasts.value).toHaveLength(0);

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

    mockCheckHealth.mockResolvedValueOnce(loadedHealth({ started_at: RESTARTED_AT }));
    await checkConnection();

    expect(engineRestarting.value).toBe(false);
    expect(activeProgressDialog.value.visible).toBe(false);
    expect(toasts.value.find(t => t.message === 'Engine restarted')).toBeTruthy();
    expect(localStorage.getItem(RESTART_LS_KEY)).toBeNull();
    expect(localStorage.getItem(RESTART_IN_FLIGHT_LS_KEY)).toBeNull();
  });

  it('keeps the restored dialog up (unchanged) when the old engine goes unreachable', async () => {
    reloadMidRestart(true);
    expect(activeProgressDialog.value.title).toBe(NEW_VERSION_TITLE);

    mockCheckHealth.mockResolvedValueOnce(unreachable);
    await checkConnection();

    expect(activeProgressDialog.value.visible).toBe(true);
    expect(activeProgressDialog.value.title).toBe(NEW_VERSION_TITLE);
    expect(engineRestarting.value).toBe(true);
  });

  it('does not hang: even a stale marker self-heals on the next connect', async () => {
    reloadMidRestart(false);

    mockCheckHealth.mockResolvedValueOnce(loadedHealth({ started_at: RESTARTED_AT }));
    await checkConnection();

    expect(engineRestarting.value).toBe(false);
    expect(localStorage.getItem(RESTART_IN_FLIGHT_LS_KEY)).toBeNull();
  });

  it('a re-sync that flips restartRequired false does NOT close the restored dialog', () => {
    reloadMidRestart(true);
    expect(activeProgressDialog.value.title).toBe(NEW_VERSION_TITLE);

    restartRequired.value = false;
    syncRestartState();

    expect(activeProgressDialog.value.visible).toBe(true);
    expect(activeProgressDialog.value.title).toBe(NEW_VERSION_TITLE);
    expect(engineRestarting.value).toBe(true);
  });

  it('cold start with only a PENDING restart (no in-flight marker) surfaces nothing', () => {
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
