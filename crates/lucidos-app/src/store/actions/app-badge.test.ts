import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { effect } from '@preact/signals';
import { unreadCount, servedUnreadNotifications } from '../store';
import type { Notification } from '../types';
import type { WorkspaceStatus } from '../../api/client/control';

const mocks = vi.hoisted(() => ({
  listWorkspaces: vi.fn(),
  listRemoteWorkspaces: vi.fn(),
  workspaceId: null as string | null,
  isPicker: false,
}));

vi.mock('../../utils/platform', async (importActual) => ({
  ...(await importActual<typeof import('../../utils/platform')>()),
  isTauri: vi.fn(() => false),
}));
vi.mock('../../api/client/control', () => ({
  listWorkspaces: mocks.listWorkspaces,
  listRemoteWorkspaces: mocks.listRemoteWorkspaces,
}));
vi.mock('../../utils/basePath', async (importActual) => {
  const actual = await importActual<typeof import('../../utils/basePath')>();
  return {
    ...actual,
    get WORKSPACE_ID() {
      return mocks.workspaceId;
    },
    get IS_PICKER() {
      return mocks.isPicker;
    },
  };
});

const {
  applyAppBadge,
  syncWorkspaceAppBadge,
  refreshOtherWorkspacesUnread,
  crossWorkspaceUnreadTotal,
  notificationsMenuUnreadTotal,
  peerWorkspaces,
  remotePeerWorkspaces,
} = await import('./app-badge');
const { isTauri } = await import('../../utils/platform');

let setAppBadge: ReturnType<typeof vi.fn>;
let clearAppBadge: ReturnType<typeof vi.fn>;

function installBadgingApi(): void {
  setAppBadge = vi.fn(() => Promise.resolve());
  clearAppBadge = vi.fn(() => Promise.resolve());
  Object.defineProperty(navigator, 'setAppBadge', { value: setAppBadge, configurable: true });
  Object.defineProperty(navigator, 'clearAppBadge', { value: clearAppBadge, configurable: true });
}

function removeBadgingApi(): void {
  Reflect.deleteProperty(navigator, 'setAppBadge');
  Reflect.deleteProperty(navigator, 'clearAppBadge');
}

function makeUnread(n: number): Notification[] {
  return Array.from({ length: n }, (_, i) => ({
    id: `u${i}`,
    title: `Notification ${i}`,
    message: `Message ${i}`,
    read: false,
    created_at: new Date().toISOString(),
  }));
}

function workspace(id: string, unread?: number): WorkspaceStatus {
  return {
    id,
    name: id,
    port: 5000,
    health: 'healthy',
    autostart: true,
    ...(unread === undefined ? {} : { unread_count: unread }),
  };
}

beforeEach(() => {
  vi.clearAllMocks();
  installBadgingApi();
  (isTauri as ReturnType<typeof vi.fn>).mockReturnValue(false);
  mocks.workspaceId = null;
  mocks.isPicker = false;
  servedUnreadNotifications.value = { status: 'not-loaded' };
  peerWorkspaces.value = [];
  remotePeerWorkspaces.value = [];
  mocks.listRemoteWorkspaces.mockResolvedValue([]);
});

afterEach(() => {
  removeBadgingApi();
});

describe('applyAppBadge', () => {
  it('sets a positive count and clears a non-positive one', () => {
    applyAppBadge(3);
    expect(setAppBadge).toHaveBeenCalledWith(3);
    expect(clearAppBadge).not.toHaveBeenCalled();

    applyAppBadge(0);
    expect(clearAppBadge).toHaveBeenCalledTimes(1);
    expect(setAppBadge).toHaveBeenCalledTimes(1);
  });

  it('no-ops when the Badging API is unavailable', () => {
    removeBadgingApi();
    expect(() => applyAppBadge(2)).not.toThrow();
  });
});

describe('syncWorkspaceAppBadge', () => {
  it('mirrors the unread set the bell projects', () => {
    servedUnreadNotifications.value = { status: 'loaded', data: makeUnread(2) };
    syncWorkspaceAppBadge();
    expect(setAppBadge).toHaveBeenCalledWith(2);
  });

  it('clears when the unread set is empty or has not loaded — the bell shows no count either', () => {
    servedUnreadNotifications.value = { status: 'loaded', data: [] };
    syncWorkspaceAppBadge();
    expect(clearAppBadge).toHaveBeenCalledTimes(1);

    servedUnreadNotifications.value = { status: 'not-loaded' };
    syncWorkspaceAppBadge();
    expect(clearAppBadge).toHaveBeenCalledTimes(2);
    expect(setAppBadge).not.toHaveBeenCalled();
  });

  it('re-asserts unconditionally — the same count written twice writes the icon twice', () => {
    servedUnreadNotifications.value = { status: 'loaded', data: [] };
    syncWorkspaceAppBadge();
    syncWorkspaceAppBadge();
    expect(clearAppBadge).toHaveBeenCalledTimes(2);
  });

  it('does not touch the web badge under Tauri (native dock badge owns it)', () => {
    (isTauri as ReturnType<typeof vi.fn>).mockReturnValue(true);
    servedUnreadNotifications.value = { status: 'loaded', data: makeUnread(4) };
    syncWorkspaceAppBadge();
    expect(setAppBadge).not.toHaveBeenCalled();
    expect(clearAppBadge).not.toHaveBeenCalled();
  });
});

describe('behind the gateway the icon badges EVERY workspace', () => {
  beforeEach(() => {
    mocks.workspaceId = 'myws';
  });

  it('sums this workspace with the others, excluding our own row', async () => {
    mocks.listWorkspaces.mockResolvedValue([
      workspace('myws', 99),
      workspace('other', 3),
      workspace('notes', 1),
    ]);
    servedUnreadNotifications.value = { status: 'loaded', data: makeUnread(2) };
    await refreshOtherWorkspacesUnread();
    expect(setAppBadge).toHaveBeenLastCalledWith(6);
  });

  it('counts a workspace with no reported count as 0 (stopped, so no engine to poll)', async () => {
    mocks.listWorkspaces.mockResolvedValue([workspace('other'), workspace('notes', 2)]);
    servedUnreadNotifications.value = { status: 'loaded', data: [] };
    await refreshOtherWorkspacesUnread();
    expect(setAppBadge).toHaveBeenLastCalledWith(2);
  });

  it('takes OUR half from the live unread set, so a mark-read lands on the same tick', async () => {
    mocks.listWorkspaces.mockResolvedValue([workspace('myws', 5), workspace('other', 3)]);
    servedUnreadNotifications.value = { status: 'loaded', data: makeUnread(5) };
    await refreshOtherWorkspacesUnread();
    expect(setAppBadge).toHaveBeenLastCalledWith(8);

    servedUnreadNotifications.value = { status: 'loaded', data: makeUnread(4) };
    syncWorkspaceAppBadge();
    expect(setAppBadge).toHaveBeenLastCalledWith(7);
    expect(mocks.listWorkspaces).toHaveBeenCalledTimes(1);
  });

  it('keeps the last-good total when the gateway blips', async () => {
    mocks.listWorkspaces.mockResolvedValue([workspace('other', 4)]);
    servedUnreadNotifications.value = { status: 'loaded', data: makeUnread(1) };
    await refreshOtherWorkspacesUnread();
    expect(setAppBadge).toHaveBeenLastCalledWith(5);

    mocks.listWorkspaces.mockRejectedValue(new Error('gateway unreachable'));
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    await expect(refreshOtherWorkspacesUnread()).resolves.toBeUndefined();
    warn.mockRestore();
    syncWorkspaceAppBadge();
    expect(setAppBadge).toHaveBeenLastCalledWith(5);
  });

  it('lets the NEWEST refresh win when two overlap', async () => {
    let releaseStale: (v: WorkspaceStatus[]) => void = () => {};
    const stale = new Promise<WorkspaceStatus[]>((resolve) => { releaseStale = resolve; });
    mocks.listWorkspaces
      .mockReturnValueOnce(stale)
      .mockResolvedValueOnce([workspace('other', 1)]);
    servedUnreadNotifications.value = { status: 'loaded', data: [] };

    const first = refreshOtherWorkspacesUnread();
    await refreshOtherWorkspacesUnread();
    expect(setAppBadge).toHaveBeenLastCalledWith(1);

    releaseStale([workspace('other', 8)]);
    await first;
    syncWorkspaceAppBadge();
    expect(setAppBadge).toHaveBeenLastCalledWith(1);
  });

  it('under Tauri still reads the peers, but writes no icon badge', async () => {
    (isTauri as ReturnType<typeof vi.fn>).mockReturnValue(true);
    mocks.listWorkspaces.mockResolvedValueOnce([workspace('other', 4)]);
    servedUnreadNotifications.value = { status: 'loaded', data: [] };
    await refreshOtherWorkspacesUnread();
    expect(mocks.listWorkspaces).toHaveBeenCalled();
    expect(setAppBadge).not.toHaveBeenCalled();
    expect(crossWorkspaceUnreadTotal.value).toBe(4);
  });

  it('reads the remote gateways beside the local ones, without moving the badge total', async () => {
    mocks.listWorkspaces.mockResolvedValue([workspace('other', 2)]);
    mocks.listRemoteWorkspaces.mockResolvedValue([
      { gateway: 'lab', state: 'reachable', workspaces: [workspace('notes', 7)] },
    ]);
    servedUnreadNotifications.value = { status: 'loaded', data: [] };
    await refreshOtherWorkspacesUnread();
    expect(remotePeerWorkspaces.value).toEqual([
      { gateway: 'lab', workspaces: [workspace('notes', 7)], unreachable: null },
    ]);
    expect(crossWorkspaceUnreadTotal.value).toBe(2);
  });

  it('keeps a silent gateway\'s last rows, marked unreachable', async () => {
    mocks.listRemoteWorkspaces.mockResolvedValueOnce([
      { gateway: 'lab', state: 'reachable', workspaces: [workspace('notes', 7)] },
    ]);
    await refreshOtherWorkspacesUnread();
    mocks.listRemoteWorkspaces.mockResolvedValueOnce([
      { gateway: 'lab', state: 'unreachable', reason: 'timed out' },
    ]);
    await refreshOtherWorkspacesUnread();
    expect(remotePeerWorkspaces.value).toMatchObject([
      { gateway: 'lab', workspaces: [workspace('notes', 7)], unreachable: 'timed out' },
    ]);
  });

  it('a failed remote listing keeps the last rows and never blocks the local counts', async () => {
    mocks.listRemoteWorkspaces.mockResolvedValueOnce([
      { gateway: 'lab', state: 'reachable', workspaces: [workspace('notes', 7)] },
    ]);
    await refreshOtherWorkspacesUnread();
    mocks.listRemoteWorkspaces.mockRejectedValueOnce(new Error('gateway unreachable'));
    mocks.listWorkspaces.mockResolvedValue([workspace('other', 4)]);
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    await expect(refreshOtherWorkspacesUnread()).resolves.toBeUndefined();
    warn.mockRestore();
    expect(remotePeerWorkspaces.value[0].workspaces).toEqual([workspace('notes', 7)]);
    expect(crossWorkspaceUnreadTotal.value).toBe(4);
  });

  it('does not reach for the control plane in the picker (it sums its own listing)', async () => {
    mocks.isPicker = true;
    await refreshOtherWorkspacesUnread();
    expect(mocks.listWorkspaces).not.toHaveBeenCalled();
    expect(mocks.listRemoteWorkspaces).not.toHaveBeenCalled();
    expect(setAppBadge).not.toHaveBeenCalled();
  });
});

describe('a bare root context badges its own workspace only', () => {
  it('never calls the control plane (a direct engine origin serves one workspace, and has no /~/ routes)', async () => {
    servedUnreadNotifications.value = { status: 'loaded', data: makeUnread(2) };
    await refreshOtherWorkspacesUnread();
    syncWorkspaceAppBadge();
    expect(mocks.listWorkspaces).not.toHaveBeenCalled();
    expect(setAppBadge).toHaveBeenLastCalledWith(2);
  });
});

describe('why the badge needs an explicit re-assert', () => {
  it('an effect on unreadCount does NOT re-run when a reload lands the same count', () => {
    servedUnreadNotifications.value = { status: 'loaded', data: [] };
    let runs = 0;
    const dispose = effect(() => {
      unreadCount.value;
      runs++;
    });
    expect(runs).toBe(1);

    servedUnreadNotifications.value = { status: 'loaded', data: [] };
    expect(runs).toBe(1);

    servedUnreadNotifications.value = { status: 'loaded', data: makeUnread(1) };
    expect(runs).toBe(2);
    dispose();
  });
});

describe('header notification total', () => {
  it('sums reachable menu rows while preserving the origin icon total', () => {
    mocks.workspaceId = 'myws';
    servedUnreadNotifications.value = { status: 'loaded', data: makeUnread(2) };
    peerWorkspaces.value = [workspace('myws', 99), workspace('notes', 3), workspace('stopped')];
    remotePeerWorkspaces.value = [
      { gateway: 'lab', unreachable: null, workspaces: [workspace('notes', 7), workspace('stopped')] },
      { gateway: 'office', unreachable: null, workspaces: [workspace('notes', 4)] },
      { gateway: 'offline', unreachable: 'timed out', workspaces: [workspace('notes', 50)] },
    ];
    expect(notificationsMenuUnreadTotal.value).toBe(16);
    expect(crossWorkspaceUnreadTotal.value).toBe(5);
    servedUnreadNotifications.value = { status: 'loaded', data: [] };
    expect(notificationsMenuUnreadTotal.value).toBe(14);
    remotePeerWorkspaces.value = [];
    expect(notificationsMenuUnreadTotal.value).toBe(3);
  });

  it('replaces the current remote route with live unreads exactly once', () => {
    mocks.workspaceId = 'notes@lab';
    servedUnreadNotifications.value = { status: 'loaded', data: makeUnread(2) };
    peerWorkspaces.value = [workspace('notes', 3)];
    remotePeerWorkspaces.value = [
      { gateway: 'lab', unreachable: null, workspaces: [workspace('notes', 99)] },
      { gateway: 'office', unreachable: null, workspaces: [workspace('notes', 4)] },
    ];
    expect(notificationsMenuUnreadTotal.value).toBe(9);
    servedUnreadNotifications.value = { status: 'loaded', data: [] };
    expect(notificationsMenuUnreadTotal.value).toBe(7);
  });
});
