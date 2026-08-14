import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { effect } from '@preact/signals';
import { unreadNotifications, unreadCount } from '../store';
import type { Notification } from '../types';
import type { WorkspaceStatus } from '../../api/client/control';

const mocks = vi.hoisted(() => ({
  listWorkspaces: vi.fn(),
  // Stand-ins for basePath's load-time consts: `workspaceId` non-null = served
  // behind the gateway under `/<slug>/` (one installed icon covers the whole
  // origin, so the badge is an aggregate); null + not picker = a legacy root /
  // direct engine port, which badges its own workspace.
  workspaceId: null as string | null,
  isPicker: false,
}));

// `isTauri` is the one context gate the sync consults at call time (the desktop
// app drives a native dock badge instead of the web Badging API). Default off.
vi.mock('../../utils/platform', async (importActual) => ({
  ...(await importActual<typeof import('../../utils/platform')>()),
  isTauri: vi.fn(() => false),
}));
vi.mock('../../api/client/control', () => ({ listWorkspaces: mocks.listWorkspaces }));
// Keep the rest of basePath real: the store pulls in the API client, which
// reads BASE_PATH at import time.
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

const { applyAppBadge, syncWorkspaceAppBadge, refreshOtherWorkspacesUnread } =
  await import('./app-badge');
const { isTauri } = await import('../../utils/platform');

let setAppBadge: ReturnType<typeof vi.fn>;
let clearAppBadge: ReturnType<typeof vi.fn>;

/** Install a fake Badging API on `navigator` (absent in the test runtime, as it
 *  is in every browser that doesn't implement it). */
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
  // Default context: a bare root (legacy engine / direct engine port), which
  // badges its own workspace and never reaches for the control plane.
  mocks.workspaceId = null;
  mocks.isPicker = false;
  unreadNotifications.value = { status: 'not-loaded' };
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
    unreadNotifications.value = { status: 'loaded', data: makeUnread(2) };
    syncWorkspaceAppBadge();
    expect(setAppBadge).toHaveBeenCalledWith(2);
  });

  it('clears when the unread set is empty or has not loaded — the bell shows no count either', () => {
    unreadNotifications.value = { status: 'loaded', data: [] };
    syncWorkspaceAppBadge();
    expect(clearAppBadge).toHaveBeenCalledTimes(1);

    unreadNotifications.value = { status: 'not-loaded' };
    syncWorkspaceAppBadge();
    expect(clearAppBadge).toHaveBeenCalledTimes(2);
    expect(setAppBadge).not.toHaveBeenCalled();
  });

  it('re-asserts unconditionally — the same count written twice writes the icon twice', () => {
    // Load-bearing, not redundant: the icon badge is an EXTERNALLY written
    // surface (iOS writes it from the push payload's `app_badge` in its parent
    // process; the SW `push` handler writes it on Chrome/Android), so the page
    // has to be able to overwrite a value it never observed changing. A sync
    // that skipped a write when its own count was unchanged would leave a
    // push-written icon reading 1 next to a bell reading 0.
    unreadNotifications.value = { status: 'loaded', data: [] };
    syncWorkspaceAppBadge();
    syncWorkspaceAppBadge();
    expect(clearAppBadge).toHaveBeenCalledTimes(2);
  });

  it('does not touch the web badge under Tauri (native dock badge owns it)', () => {
    (isTauri as ReturnType<typeof vi.fn>).mockReturnValue(true);
    unreadNotifications.value = { status: 'loaded', data: makeUnread(4) };
    syncWorkspaceAppBadge();
    expect(setAppBadge).not.toHaveBeenCalled();
    expect(clearAppBadge).not.toHaveBeenCalled();
  });
});

describe('behind the gateway the icon badges EVERY workspace', () => {
  // The gateway re-stamps every manifest it serves with `scope: "/"`, so one
  // installed icon covers the picker and every `/<slug>/` on the origin. Badging
  // it with whichever workspace is on screen hid every other workspace's
  // unreads, which is the bug this block pins.
  beforeEach(() => {
    mocks.workspaceId = 'myws';
  });

  it('sums this workspace with the others, excluding our own row', async () => {
    mocks.listWorkspaces.mockResolvedValue([
      workspace('myws', 99), // ignored: our own row is the LIVE count below
      workspace('other', 3),
      workspace('notes', 1),
    ]);
    unreadNotifications.value = { status: 'loaded', data: makeUnread(2) };
    await refreshOtherWorkspacesUnread();
    expect(setAppBadge).toHaveBeenLastCalledWith(6);
  });

  it('counts a workspace with no reported count as 0 (stopped, so no engine to poll)', async () => {
    mocks.listWorkspaces.mockResolvedValue([workspace('other'), workspace('notes', 2)]);
    unreadNotifications.value = { status: 'loaded', data: [] };
    await refreshOtherWorkspacesUnread();
    expect(setAppBadge).toHaveBeenLastCalledWith(2);
  });

  it('takes OUR half from the live unread set, so a mark-read lands on the same tick', async () => {
    // Load-bearing: the polled listing still reports our pre-read count for
    // seconds after an optimistic mark-read (the gateway's supervise loop has
    // not re-probed us). Reading our own row from it would leave the icon
    // disagreeing with the bell about the workspace on screen.
    mocks.listWorkspaces.mockResolvedValue([workspace('myws', 5), workspace('other', 3)]);
    unreadNotifications.value = { status: 'loaded', data: makeUnread(5) };
    await refreshOtherWorkspacesUnread();
    expect(setAppBadge).toHaveBeenLastCalledWith(8);

    // One read, no refetch: 4 of ours + the same 3 elsewhere.
    unreadNotifications.value = { status: 'loaded', data: makeUnread(4) };
    syncWorkspaceAppBadge();
    expect(setAppBadge).toHaveBeenLastCalledWith(7);
    expect(mocks.listWorkspaces).toHaveBeenCalledTimes(1);
  });

  it('keeps the last-good total when the gateway blips', async () => {
    mocks.listWorkspaces.mockResolvedValue([workspace('other', 4)]);
    unreadNotifications.value = { status: 'loaded', data: makeUnread(1) };
    await refreshOtherWorkspacesUnread();
    expect(setAppBadge).toHaveBeenLastCalledWith(5);

    // Best-effort: a failed refresh must not zero the others' half (which would
    // read as "everything elsewhere was read") and must not throw.
    mocks.listWorkspaces.mockRejectedValue(new Error('gateway unreachable'));
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    await expect(refreshOtherWorkspacesUnread()).resolves.toBeUndefined();
    warn.mockRestore();
    syncWorkspaceAppBadge();
    expect(setAppBadge).toHaveBeenLastCalledWith(5);
  });

  it('lets the NEWEST refresh win when two overlap', async () => {
    // Startup, resume and the interval can all be in flight at once, and the
    // gateway listing takes as long as its slowest stack lock. Without the seq
    // guard the slower EARLIER response lands last and pins the icon to stale
    // counts until the next refresh.
    let releaseStale: (v: WorkspaceStatus[]) => void = () => {};
    const stale = new Promise<WorkspaceStatus[]>((resolve) => { releaseStale = resolve; });
    mocks.listWorkspaces
      .mockReturnValueOnce(stale)
      .mockResolvedValueOnce([workspace('other', 1)]);
    unreadNotifications.value = { status: 'loaded', data: [] };

    const first = refreshOtherWorkspacesUnread();
    await refreshOtherWorkspacesUnread();
    expect(setAppBadge).toHaveBeenLastCalledWith(1);

    releaseStale([workspace('other', 8)]);
    await first;
    // The superseded listing is dropped, not applied.
    syncWorkspaceAppBadge();
    expect(setAppBadge).toHaveBeenLastCalledWith(1);
  });

  it('does not reach for the control plane under Tauri (the native tray owns the total)', async () => {
    (isTauri as ReturnType<typeof vi.fn>).mockReturnValue(true);
    await refreshOtherWorkspacesUnread();
    expect(mocks.listWorkspaces).not.toHaveBeenCalled();
    expect(setAppBadge).not.toHaveBeenCalled();
  });

  it('does not reach for the control plane in the picker (it sums its own listing)', async () => {
    mocks.isPicker = true;
    await refreshOtherWorkspacesUnread();
    expect(mocks.listWorkspaces).not.toHaveBeenCalled();
    expect(setAppBadge).not.toHaveBeenCalled();
  });
});

describe('a bare root context badges its own workspace only', () => {
  it('never calls the control plane (a direct engine origin serves one workspace, and has no /~/ routes)', async () => {
    unreadNotifications.value = { status: 'loaded', data: makeUnread(2) };
    await refreshOtherWorkspacesUnread();
    syncWorkspaceAppBadge();
    expect(mocks.listWorkspaces).not.toHaveBeenCalled();
    expect(setAppBadge).toHaveBeenLastCalledWith(2);
  });
});

describe('why the badge needs an explicit re-assert', () => {
  it('an effect on unreadCount does NOT re-run when a reload lands the same count', () => {
    // The signals cutoff: a computed whose recomputed value is equal does not
    // notify its subscribers. So the `unreadCount` effect in store/effects.ts
    // covers only the CHANGE case — a resume-time reload landing the same count
    // re-runs nothing, and an icon badge written behind the page's back (by a
    // push) would never be corrected. This test pins the mechanism so nobody
    // "simplifies" the explicit re-asserts back out.
    unreadNotifications.value = { status: 'loaded', data: [] };
    let runs = 0;
    const dispose = effect(() => {
      unreadCount.value;
      runs++;
    });
    expect(runs).toBe(1);

    // A fresh array with the same length — the signal changed, the count didn't.
    unreadNotifications.value = { status: 'loaded', data: [] };
    expect(runs).toBe(1);

    // A real change does re-run it.
    unreadNotifications.value = { status: 'loaded', data: makeUnread(1) };
    expect(runs).toBe(2);
    dispose();
  });
});
