import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';

const mocks = vi.hoisted(() => ({
  isTauri: vi.fn(() => true),
  checkAppUpdate: vi.fn(),
  installAppUpdateAndRestart: vi.fn(),
  showToast: vi.fn(),
}));

// The persistent Settings → System surface reads these, so the tests assert on
// the values that page would actually render. A `.value` box is all the action
// touches, and it avoids importing the real store (and its whole dependency
// graph) into a unit test.
const storeSignals = vi.hoisted(() => ({
  latestTauriAppVersion: { value: null as string | null },
  appUpdateCheckError: { value: null as string | null },
}));

vi.mock('../../utils/platform', () => ({ isTauri: mocks.isTauri }));
vi.mock('../../utils/tauri', () => ({
  checkAppUpdate: mocks.checkAppUpdate,
  installAppUpdateAndRestart: mocks.installAppUpdateAndRestart,
}));
vi.mock('../store', () => ({
  showToast: mocks.showToast,
  latestTauriAppVersion: storeSignals.latestTauriAppVersion,
  appUpdateCheckError: storeSignals.appUpdateCheckError,
}));

const { checkForAppUpdate } = await import('./app-update');

beforeEach(() => {
  mocks.isTauri.mockReturnValue(true);
  mocks.checkAppUpdate.mockReset();
  mocks.installAppUpdateAndRestart.mockReset();
  mocks.showToast.mockReset();
  storeSignals.latestTauriAppVersion.value = null;
  storeSignals.appUpdateCheckError.value = null;
});

afterEach(() => vi.restoreAllMocks());

describe('checkForAppUpdate', () => {
  it('is a no-op outside the Tauri client (browser / PWA / dev)', async () => {
    mocks.isTauri.mockReturnValue(false);
    await checkForAppUpdate();
    expect(mocks.checkAppUpdate).not.toHaveBeenCalled();
    expect(mocks.showToast).not.toHaveBeenCalled();
  });

  it('shows no toast when there is no update', async () => {
    mocks.checkAppUpdate.mockResolvedValue(null);
    await checkForAppUpdate();
    expect(mocks.checkAppUpdate).toHaveBeenCalledTimes(1);
    expect(mocks.showToast).not.toHaveBeenCalled();
  });

  it('surfaces the in-app "Update & restart" toast when an update is available', async () => {
    mocks.checkAppUpdate.mockResolvedValue('2026.6.25');
    await checkForAppUpdate();
    expect(mocks.showToast).toHaveBeenCalledTimes(1);
    const [message, type, opts] = mocks.showToast.mock.calls[0];
    expect(message).toContain('2026.6.25');
    expect(type).toBe('info');
    expect(opts.key).toBe('app-update-available');
    expect(opts.action.label).toBe('Update & restart');
  });

  it('clicking the toast action installs the update + restarts the stack', async () => {
    mocks.checkAppUpdate.mockResolvedValue('2026.6.25');
    mocks.installAppUpdateAndRestart.mockResolvedValue(undefined);
    await checkForAppUpdate();
    const opts = mocks.showToast.mock.calls[0][2];
    opts.action.onClick();
    expect(mocks.installAppUpdateAndRestart).toHaveBeenCalledTimes(1);
  });

  it('swallows a failed check (best-effort) — no toast, retried next poll', async () => {
    mocks.checkAppUpdate.mockRejectedValue(new Error('network'));
    await checkForAppUpdate();
    expect(mocks.showToast).not.toHaveBeenCalled();
  });

  // The toast is transient; Settings → System is the surface that persists, so
  // the outcome has to be RECORDED, not just announced.
  it('records the available version for the persistent System surface', async () => {
    mocks.checkAppUpdate.mockResolvedValue('0.16.0');
    await checkForAppUpdate();
    expect(storeSignals.latestTauriAppVersion.value).toBe('0.16.0');
    expect(storeSignals.appUpdateCheckError.value).toBeNull();
  });

  // A silent failure is what made a stranded install indistinguishable from an
  // up-to-date one — the whole point of recording it.
  it('records why a check failed instead of only console.warn-ing', async () => {
    mocks.checkAppUpdate.mockRejectedValue(new Error('network'));
    await checkForAppUpdate();
    expect(storeSignals.appUpdateCheckError.value).toContain('network');
    expect(mocks.showToast).not.toHaveBeenCalled();
  });

  // In a Tauri DEV client `check_app_update` is a no-op returning null, which is
  // indistinguishable from "up to date". Assigning that null blindly would wipe
  // the version connection.ts reads from the engine's /health — dev's only
  // source — and the two would fight on every poll.
  it('does not clobber a version it did not set', async () => {
    mocks.checkAppUpdate.mockResolvedValue(null);
    await checkForAppUpdate(); // relinquish ownership if an earlier case took it
    storeSignals.latestTauriAppVersion.value = '2026.07.03.0'; // as if from /health
    await checkForAppUpdate();
    expect(storeSignals.latestTauriAppVersion.value).toBe('2026.07.03.0');
  });

  it('does clear the version it set once the update is gone', async () => {
    mocks.checkAppUpdate.mockResolvedValue('0.16.0');
    await checkForAppUpdate();
    expect(storeSignals.latestTauriAppVersion.value).toBe('0.16.0');

    mocks.checkAppUpdate.mockReset();
    mocks.checkAppUpdate.mockResolvedValue(null);
    await checkForAppUpdate();
    expect(storeSignals.latestTauriAppVersion.value).toBeNull();
  });

  it('clears a previous error once a check succeeds again', async () => {
    mocks.checkAppUpdate.mockRejectedValue(new Error('network'));
    await checkForAppUpdate();
    expect(storeSignals.appUpdateCheckError.value).not.toBeNull();

    mocks.checkAppUpdate.mockReset();
    mocks.checkAppUpdate.mockResolvedValue(null);
    await checkForAppUpdate();
    expect(storeSignals.appUpdateCheckError.value).toBeNull();
    expect(storeSignals.latestTauriAppVersion.value).toBeNull();
  });
});

describe('startAppUpdateChecks', () => {
  // The regression this exists to prevent: the old guard returned early when a
  // timer already existed, so only the FIRST workspace mount of a client process
  // ever checked. With a 6h interval behind it, an update published mid-session
  // stayed invisible until the app was fully quit and relaunched.
  it('re-checks on every mount, not just the first of a client process', async () => {
    const { startAppUpdateChecks, stopAppUpdateChecks } = await import('./app-update');
    mocks.checkAppUpdate.mockResolvedValue(null);
    try {
      startAppUpdateChecks();
      startAppUpdateChecks();
      startAppUpdateChecks();
      expect(mocks.checkAppUpdate).toHaveBeenCalledTimes(3);
    } finally {
      stopAppUpdateChecks();
    }
  });

  it('does not stack a second interval when called again', async () => {
    const { startAppUpdateChecks, stopAppUpdateChecks } = await import('./app-update');
    mocks.checkAppUpdate.mockResolvedValue(null);
    const setInterval = vi.spyOn(globalThis, 'setInterval');
    try {
      startAppUpdateChecks();
      startAppUpdateChecks();
      expect(setInterval).toHaveBeenCalledTimes(1);
    } finally {
      stopAppUpdateChecks();
    }
  });

  it('stays a no-op outside the Tauri client', async () => {
    const { startAppUpdateChecks } = await import('./app-update');
    mocks.isTauri.mockReturnValue(false);
    startAppUpdateChecks();
    expect(mocks.checkAppUpdate).not.toHaveBeenCalled();
  });
});
