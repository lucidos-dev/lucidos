import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
import type { AppUpdateProgress, AppUpdateRunning } from '../../utils/tauri';
import type { UpdateRelay, ReleaseCheck, ReleaseOffer } from '../../api/client/control';
import type { RelayedUpdate } from '../store';
import type { UpdateRoute } from './app-update';
import { appUpdateNarration, appUpdateDialogState } from '../progressDialogCopy';

const mocks = vi.hoisted(() => ({
  isTauri: vi.fn(() => true),
  thisDeviceIsMobile: vi.fn(() => false),
  checkAppUpdate: vi.fn(),
  installAppUpdateAndRestart: vi.fn(),
  cancelAppUpdate: vi.fn(),
  listen: vi.fn(),
  showToast: vi.fn(),
  removeToast: vi.fn(),
  openWhatsNew: vi.fn(),
  openSettingsSubview: vi.fn(),
  requestUpdateCheck: vi.fn(),
  getGatewayStatus: vi.fn(),
  requestUpdateRelay: vi.fn(),
}));

function releaseCheckOf(
  latest: Partial<ReleaseOffer> | null = null,
  over: Partial<ReleaseCheck> = {},
): ReleaseCheck {
  return {
    enabled: true,
    supported: true,
    current_version: '1.2.3',
    checked_at: '2026-08-23T10:00:00Z',
    last_error: null,
    latest: latest
      ? { version: '9.9.9', notes: null, install: null, command: null, ...latest }
      : null,
    update_address: null,
    update_address_error: null,
    ...over,
  };
}

const storeSignals = vi.hoisted(() => ({
  latestTauriAppVersion: { value: null as string | null },
  clientOfferedRelease: { value: null as string | null },
  latestTauriAppNotes: { value: null as string | null },
  appUpdateCheckError: { value: null as string | null },
  appUpdateCheckInFlight: { value: false },
  appUpdateProgress: { value: null as AppUpdateProgress | null },
  releaseCheck: { value: null as ReleaseCheck | null },
  settingsScrollTarget: { value: null as string | null },
  lucidosRelease: { value: null as string | null },
  updateRelay: { value: null as UpdateRelay | null },
  relayedUpdate: { value: null as RelayedUpdate | null },
}));

function offer(version: string, notes: string | null = null) {
  return { version, notes };
}

const ROUTES: UpdateRoute[] = ['install', 'relay', 'check', 'guide', 'desktop'];

vi.mock('../../utils/platform', () => ({
  isTauri: mocks.isTauri,
  thisDeviceIsMobile: mocks.thisDeviceIsMobile,
}));
vi.mock('./menu', () => ({
  openWhatsNew: mocks.openWhatsNew,
  openSettingsSubview: mocks.openSettingsSubview,
}));
vi.mock('../../api/client/control', () => ({
  requestUpdateCheck: mocks.requestUpdateCheck,
  getGatewayStatus: mocks.getGatewayStatus,
  requestUpdateRelay: mocks.requestUpdateRelay,
}));
vi.mock('../../utils/tauri', async (importOriginal) => ({
  checkAppUpdate: mocks.checkAppUpdate,
  installAppUpdateAndRestart: mocks.installAppUpdateAndRestart,
  cancelAppUpdate: mocks.cancelAppUpdate,
  listen: mocks.listen,
  APP_UPDATE_PROGRESS_EVENT:
    (await importOriginal<typeof import('../../utils/tauri')>()).APP_UPDATE_PROGRESS_EVENT,
}));
vi.mock('../store', () => ({
  showToast: mocks.showToast,
  removeToast: mocks.removeToast,
  latestTauriAppVersion: storeSignals.latestTauriAppVersion,
  clientOfferedRelease: storeSignals.clientOfferedRelease,
  latestTauriAppNotes: storeSignals.latestTauriAppNotes,
  appUpdateCheckError: storeSignals.appUpdateCheckError,
  appUpdateCheckInFlight: storeSignals.appUpdateCheckInFlight,
  appUpdateProgress: storeSignals.appUpdateProgress,
  releaseCheck: storeSignals.releaseCheck,
  settingsScrollTarget: storeSignals.settingsScrollTarget,
  lucidosRelease: storeSignals.lucidosRelease,
  updateRelay: storeSignals.updateRelay,
  relayedUpdate: storeSignals.relayedUpdate,
}));

const {
  canCheckForUpdatesHere,
  canInstallUpdateHere,
  checkAppUpdateViaClient,
  checkForUpdatesNow,
  followUpdateRoute,
  installAppUpdate,
  refreshReleaseCheck,
  reportUpdateCheck,
  shadowedEngine,
  startAppUpdateProgress,
  stopAppUpdateProgress,
  updateControlLabel,
  updateRoute,
  relayUpdateToDesktop,
} = await import('./app-update');
const { packagedUpdateVersion } = await import('../packagedUpdate');

let emitProgress: (frame: AppUpdateProgress) => void = () => {
  throw new Error('startAppUpdateProgress() must run before a frame can be emitted');
};

function lastToastOpts(): Record<string, unknown> {
  const calls = mocks.showToast.mock.calls;
  return (calls[calls.length - 1]?.[2] ?? {}) as Record<string, unknown>;
}

function lastToast(): { message: string; type: string; opts: Record<string, unknown> } {
  const call = mocks.showToast.mock.calls[mocks.showToast.mock.calls.length - 1];
  return { message: call[0] as string, type: call[1] as string, opts: lastToastOpts() };
}

beforeEach(() => {
  mocks.isTauri.mockReturnValue(true);
  mocks.thisDeviceIsMobile.mockReturnValue(false);
  mocks.checkAppUpdate.mockReset();
  mocks.installAppUpdateAndRestart.mockReset();
  mocks.cancelAppUpdate.mockReset();
  mocks.cancelAppUpdate.mockResolvedValue(undefined);
  mocks.showToast.mockReset();
  mocks.removeToast.mockReset();
  mocks.openWhatsNew.mockReset();
  mocks.openSettingsSubview.mockReset();
  mocks.requestUpdateCheck.mockReset();
  mocks.requestUpdateCheck.mockResolvedValue(releaseCheckOf(null));
  mocks.listen.mockReset();
  mocks.listen.mockImplementation((_event: string, handler: (e: { payload: AppUpdateProgress }) => void) => {
    emitProgress = (frame) => handler({ payload: frame });
    return Promise.resolve(() => {});
  });
  storeSignals.latestTauriAppVersion.value = null;
  storeSignals.clientOfferedRelease.value = null;
  storeSignals.latestTauriAppNotes.value = null;
  storeSignals.appUpdateCheckError.value = null;
  storeSignals.appUpdateCheckInFlight.value = false;
  storeSignals.appUpdateProgress.value = null;
  storeSignals.releaseCheck.value = null;
  storeSignals.updateRelay.value = null;
  storeSignals.relayedUpdate.value = null;
  localStorage.clear();
  mocks.getGatewayStatus.mockReset();
  mocks.getGatewayStatus.mockResolvedValue({ build_id: 'b', update_available: false });
  mocks.requestUpdateRelay.mockReset();
  storeSignals.settingsScrollTarget.value = null;
  storeSignals.lucidosRelease.value = null;
  delete (globalThis as { __LUCIDOS_APP_VERSION__?: string }).__LUCIDOS_APP_VERSION__;
  delete (globalThis as { __LUCIDOS_APP_RELEASE__?: string }).__LUCIDOS_APP_RELEASE__;
});

afterEach(() => {
  stopAppUpdateProgress();
  vi.restoreAllMocks();
});

describe('appUpdateNarration', () => {
  it('names the version and both byte counts for a sized download', () => {
    const narration = appUpdateNarration({
      version: '2026.7.30',
      phase: 'downloading',
      downloaded: 52_428_800,
      total: 104_857_600,
    });
    expect(narration.message).toContain('Lucidos 2026.7.30');
    expect(narration.message).toContain('50 MB');
    expect(narration.message).toContain('100 MB');
    expect(narration.progress).toBeCloseTo(0.5);
  });

  it('reports bytes without inventing a total when the size is unknown', () => {
    const narration = appUpdateNarration({
      version: '2026.7.30',
      phase: 'downloading',
      downloaded: 52_428_800,
      total: null,
    });
    expect(narration.progress).toBeNull();
    expect(narration.message).toContain('50 MB');
    expect(narration.message).not.toContain('of');
    expect(narration.message).not.toContain('NaN');
  });

  it('clamps a download that overruns the size the server declared', () => {
    const narration = appUpdateNarration({
      version: '2026.7.30',
      phase: 'downloading',
      downloaded: 120,
      total: 100,
    });
    expect(narration.progress).toBe(1);
  });

  it('survives a zero total without dividing by it', () => {
    const narration = appUpdateNarration({
      version: '2026.7.30',
      phase: 'downloading',
      downloaded: 0,
      total: 0,
    });
    expect(narration.progress).toBeNull();
  });

  it('offers cancel only while the run can still be abandoned', () => {
    const cancellableByPhase: Array<[AppUpdateRunning, boolean]> = [
      [{ version: null, phase: 'checking' }, true],
      [{ version: 'v', phase: 'downloading', downloaded: 1, total: 2 }, true],
      [{ version: 'v', phase: 'verifying' }, false],
      [{ version: 'v', phase: 'installing' }, false],
      [{ version: 'v', phase: 'restarting-services' }, false],
      [{ version: 'v', phase: 'relaunching' }, false],
    ];
    for (const [frame, cancellable] of cancellableByPhase) {
      expect(appUpdateNarration(frame).cancellable, frame.phase).toBe(cancellable);
    }
  });

  it('gives every in-flight phase a sentence', () => {
    const frames: AppUpdateRunning[] = [
      { version: null, phase: 'checking' },
      { version: 'v', phase: 'downloading', downloaded: 1, total: 2 },
      { version: 'v', phase: 'verifying' },
      { version: 'v', phase: 'installing' },
      { version: 'v', phase: 'restarting-services' },
      { version: 'v', phase: 'relaunching' },
    ];
    for (const frame of frames) {
      expect(appUpdateNarration(frame).message, frame.phase).not.toBe('');
    }
  });

  it('falls back to a version-less label before the check resolves one', () => {
    expect(appUpdateNarration({ version: null, phase: 'installing' }).message)
      .toBe('Installing the update…');
  });
});

describe('update progress narration', () => {
  const dialogNow = () => appUpdateDialogState(
    storeSignals.appUpdateProgress.value as AppUpdateRunning,
    () => {},
  );

  it('narrates on the click rather than waiting for the first event', async () => {
    storeSignals.latestTauriAppVersion.value = '2026.7.30';
    let atInvoke: AppUpdateProgress | null = null;
    mocks.installAppUpdateAndRestart.mockImplementation(() => {
      atInvoke = storeSignals.appUpdateProgress.value;
      return Promise.resolve(undefined);
    });
    await installAppUpdate();
    expect(appUpdateDialogState(atInvoke as unknown as AppUpdateRunning, () => {}).message)
      .toBe('Checking for updates…');
  });

  it('records a determinate frame, which the dialog paints as a bar', () => {
    startAppUpdateProgress();
    emitProgress({ version: '2026.7.30', phase: 'downloading', downloaded: 50, total: 200 });
    const dialog = dialogNow();
    expect(dialog.message).toContain('Downloading Lucidos 2026.7.30');
    expect(dialog.progress).toBeCloseTo(0.25);
    expect(dialog.cancel!.label).toBe('Cancel');
  });

  it('offers no cancel for a run relayed to the desktop app', () => {
    const frame = { version: '2026.7.30', phase: 'downloading' as const, downloaded: 50, total: 200 };
    expect(appUpdateDialogState(frame, null).cancel).toBeUndefined();
  });

  it('raises no toast for a running frame, and clears the offer behind it', () => {
    startAppUpdateProgress();
    emitProgress({ version: '2026.7.30', phase: 'downloading', downloaded: 50, total: 200 });
    expect(mocks.showToast).not.toHaveBeenCalled();
    expect(mocks.removeToast).toHaveBeenCalledWith('app-update-available');
  });

  it('drops the cancel affordance once the run has committed', () => {
    startAppUpdateProgress();
    emitProgress({ version: '2026.7.30', phase: 'installing' });
    const dialog = dialogNow();
    expect(dialog.cancel).toBeUndefined();
    expect(dialog.progress).toBeNull();
  });

  it('clears the run before the failure toast is shown', () => {
    const seen: unknown[] = [];
    mocks.showToast.mockImplementation(() => { seen.push(storeSignals.appUpdateProgress.value); });
    startAppUpdateProgress();
    emitProgress({ version: '2026.7.30', phase: 'downloading', downloaded: 1, total: 2 });
    emitProgress({ version: '2026.7.30', phase: 'failed', message: 'signature mismatch' });
    expect(seen).toEqual([null]);
  });

  it('reports a failure with its reason and leaves nothing spinning', () => {
    startAppUpdateProgress();
    emitProgress({ version: '2026.7.30', phase: 'downloading', downloaded: 1, total: 2 });
    emitProgress({ version: '2026.7.30', phase: 'failed', message: 'signature mismatch' });
    const { message, type, opts } = lastToast();
    expect(type).toBe('error');
    expect(message).toContain('signature mismatch');
    expect(opts.spinning).toBeFalsy();
    expect(storeSignals.appUpdateProgress.value).toBeNull();
  });

  it('shows a bundle-swap failure verbatim, with no "Update failed" prefix', () => {
    startAppUpdateProgress();
    emitProgress({ version: '2026.7.30', phase: 'installing' });
    emitProgress({
      version: '2026.7.30',
      phase: 'bundle-swap-failed',
      message: 'The update failed and no runnable app is at /Applications/Lucidos.app: '
        + 'the application bundle is gone. Reinstall Lucidos from the .dmg to recover.',
    });
    const { message, type, opts } = lastToast();
    expect(type).toBe('error');
    expect(message).not.toContain('Update failed');
    expect(message).toContain('Reinstall Lucidos from the .dmg');
    expect(opts.spinning).toBeFalsy();
    expect(storeSignals.appUpdateProgress.value).toBeNull();
  });

  it('does not re-offer the update after a bundle-swap failure', () => {
    startAppUpdateProgress();
    emitProgress({ version: '2026.7.30', phase: 'installing' });
    emitProgress({ version: '2026.7.30', phase: 'bundle-swap-failed', message: 'gone' });
    expect(lastToast().opts.action).toBeUndefined();
  });

  it('re-offers the update after a cancel', () => {
    startAppUpdateProgress();
    emitProgress({ version: '2026.7.30', phase: 'downloading', downloaded: 1, total: 2 });
    emitProgress({ version: '2026.7.30', phase: 'cancelled' });
    const { message, opts } = lastToast();
    expect(message).toBe('Lucidos 2026.7.30 available');
    expect((opts.action as { label: string }).label).toBe('Update & restart');
    expect(storeSignals.appUpdateProgress.value).toBeNull();
  });

  it('keeps one surface for the whole run, whatever the phase', () => {
    startAppUpdateProgress();
    emitProgress({ version: '2026.7.30', phase: 'checking' });
    emitProgress({ version: '2026.7.30', phase: 'downloading', downloaded: 1, total: 2 });
    emitProgress({ version: '2026.7.30', phase: 'installing' });
    expect(storeSignals.appUpdateProgress.value).toEqual({ version: '2026.7.30', phase: 'installing' });
    expect(mocks.showToast).not.toHaveBeenCalled();
  });

  it('never leaves the dialog up when the invoke itself rejects', async () => {
    storeSignals.latestTauriAppVersion.value = '2026.7.30';
    mocks.installAppUpdateAndRestart.mockRejectedValue('ipc unavailable');
    await installAppUpdate();
    const { message, type } = lastToast();
    expect(type).toBe('error');
    expect(message).toContain('ipc unavailable');
    expect(storeSignals.appUpdateProgress.value).toBeNull();
  });

  it('clears a run the install left behind once it resolves without restarting', async () => {
    storeSignals.latestTauriAppVersion.value = '2026.7.30';
    mocks.installAppUpdateAndRestart.mockResolvedValue(undefined);
    startAppUpdateProgress();
    emitProgress({ version: '2026.7.30', phase: 'downloading', downloaded: 1, total: 2 });
    await installAppUpdate();
    expect(storeSignals.appUpdateProgress.value).toBeNull();
    expect(mocks.removeToast).toHaveBeenCalledWith('app-update-available');
  });

  it('lets a second install run after the first resolves without restarting', async () => {
    mocks.installAppUpdateAndRestart.mockResolvedValue(undefined);
    await installAppUpdate();
    await installAppUpdate();
    expect(mocks.installAppUpdateAndRestart).toHaveBeenCalledTimes(2);
  });

  it('does not let the periodic check clobber a live run', async () => {
    storeSignals.appUpdateProgress.value = { version: '2026.7.30', phase: 'installing' };
    await checkAppUpdateViaClient();
    expect(mocks.checkAppUpdate).not.toHaveBeenCalled();
    expect(mocks.showToast).not.toHaveBeenCalled();
  });

  it('subscribes once however many times the workspace remounts', () => {
    startAppUpdateProgress();
    startAppUpdateProgress();
    startAppUpdateProgress();
    expect(mocks.listen).toHaveBeenCalledTimes(1);
  });
});

describe('checkAppUpdateViaClient', () => {
  it('is a no-op outside the Tauri client (browser / PWA / dev)', async () => {
    mocks.isTauri.mockReturnValue(false);
    await checkAppUpdateViaClient();
    expect(mocks.checkAppUpdate).not.toHaveBeenCalled();
    expect(mocks.showToast).not.toHaveBeenCalled();
  });

  it('shows no toast when there is no update', async () => {
    mocks.checkAppUpdate.mockResolvedValue(null);
    await checkAppUpdateViaClient();
    expect(mocks.checkAppUpdate).toHaveBeenCalledTimes(1);
    expect(mocks.showToast).not.toHaveBeenCalled();
  });

  it('surfaces the in-app "Update & restart" toast when an update is available', async () => {
    mocks.checkAppUpdate.mockResolvedValue(offer('2026.6.25'));
    await checkAppUpdateViaClient();
    expect(mocks.showToast).toHaveBeenCalledTimes(1);
    const [message, type, opts] = mocks.showToast.mock.calls[0];
    expect(message).toContain('2026.6.25');
    expect(type).toBe('info');
    expect(opts.key).toBe('app-update-available');
    expect(opts.action.label).toBe('Update & restart');
  });

  it('offers the in-app update as a confirm action, never a neutral one', async () => {
    mocks.checkAppUpdate.mockResolvedValue(offer('2026.6.25'));
    await checkAppUpdateViaClient();
    const [, , opts] = mocks.showToast.mock.calls[0];
    expect(opts.action.variant).toBe('confirm');
  });

  it('clicking the toast action installs the update + restarts the stack', async () => {
    mocks.checkAppUpdate.mockResolvedValue(offer('2026.6.25'));
    mocks.installAppUpdateAndRestart.mockResolvedValue(undefined);
    await checkAppUpdateViaClient();
    const opts = mocks.showToast.mock.calls[0][2];
    opts.action.onClick();
    expect(mocks.installAppUpdateAndRestart).toHaveBeenCalledTimes(1);
  });

  it("keeps the offered release's notes beside the version they describe", async () => {
    mocks.checkAppUpdate.mockResolvedValue(offer('2026.6.25', '### Added\n\n- a thing'));
    await checkAppUpdateViaClient();
    expect(storeSignals.clientOfferedRelease.value).toBe('2026.6.25');
    expect(storeSignals.latestTauriAppNotes.value).toBe('### Added\n\n- a thing');
  });

  it('offers a way to read them, which opens the release it just announced', async () => {
    mocks.checkAppUpdate.mockResolvedValue(offer('2026.6.25', '### Added\n\n- a thing'));
    await checkAppUpdateViaClient();
    const opts = mocks.showToast.mock.calls[0][2];
    expect(opts.secondaryAction.label).toBe("What's new");
    opts.secondaryAction.onClick();
    expect(mocks.openWhatsNew).toHaveBeenCalledWith('2026.6.25');
    expect(mocks.installAppUpdateAndRestart).not.toHaveBeenCalled();
  });

  it('re-offers after a cancel with the version still named', async () => {
    mocks.checkAppUpdate.mockResolvedValue(offer('2026.6.25', '### Added\n\n- a thing'));
    startAppUpdateProgress();
    await vi.waitFor(() => expect(mocks.listen).toHaveBeenCalled());
    await checkAppUpdateViaClient();
    emitProgress({ version: '2026.6.25', phase: 'cancelled' });
    const { secondaryAction } = lastToast().opts as { secondaryAction: { onClick: () => void } };
    secondaryAction.onClick();
    expect(mocks.openWhatsNew).toHaveBeenCalledWith('2026.6.25');
  });

  it('offers no way to read notes the manifest never carried', async () => {
    mocks.checkAppUpdate.mockResolvedValue(offer('2026.6.25'));
    await checkAppUpdateViaClient();
    expect(mocks.showToast.mock.calls[0][2].secondaryAction).toBeUndefined();
  });

  it('drops the notes with the version when the update goes away', async () => {
    mocks.checkAppUpdate.mockResolvedValue(offer('2026.6.25', '### Added'));
    await checkAppUpdateViaClient();
    mocks.checkAppUpdate.mockResolvedValue(null);
    await checkAppUpdateViaClient();
    expect(storeSignals.clientOfferedRelease.value).toBe(null);
    expect(storeSignals.latestTauriAppNotes.value).toBe(null);
  });

  it('swallows a failed check (best-effort) — no toast, retried next poll', async () => {
    mocks.checkAppUpdate.mockRejectedValue(new Error('network'));
    await checkAppUpdateViaClient();
    expect(mocks.showToast).not.toHaveBeenCalled();
  });

  it('records the available version for the persistent System surface', async () => {
    mocks.checkAppUpdate.mockResolvedValue(offer('0.16.0'));
    await checkAppUpdateViaClient();
    expect(storeSignals.clientOfferedRelease.value).toBe('0.16.0');
    expect(storeSignals.appUpdateCheckError.value).toBeNull();
  });

  it('records why a check failed instead of only console.warn-ing', async () => {
    mocks.checkAppUpdate.mockRejectedValue(new Error('network'));
    await checkAppUpdateViaClient();
    expect(storeSignals.appUpdateCheckError.value).toContain('network');
    expect(mocks.showToast).not.toHaveBeenCalled();
  });

  it("never touches the engine's build id", async () => {
    storeSignals.latestTauriAppVersion.value = '2026.07.03.0';
    mocks.checkAppUpdate.mockResolvedValue(offer('0.16.0'));
    await checkAppUpdateViaClient();
    mocks.checkAppUpdate.mockResolvedValue(null);
    await checkAppUpdateViaClient();
    expect(storeSignals.latestTauriAppVersion.value).toBe('2026.07.03.0');
  });

  it('clears the release it offered once the update is gone', async () => {
    mocks.checkAppUpdate.mockResolvedValue(offer('0.16.0'));
    await checkAppUpdateViaClient();
    expect(storeSignals.clientOfferedRelease.value).toBe('0.16.0');

    mocks.checkAppUpdate.mockReset();
    mocks.checkAppUpdate.mockResolvedValue(null);
    await checkAppUpdateViaClient();
    expect(storeSignals.clientOfferedRelease.value).toBeNull();
  });

  it('clears a previous error once a check succeeds again', async () => {
    mocks.checkAppUpdate.mockRejectedValue(new Error('network'));
    await checkAppUpdateViaClient();
    expect(storeSignals.appUpdateCheckError.value).not.toBeNull();

    mocks.checkAppUpdate.mockReset();
    mocks.checkAppUpdate.mockResolvedValue(null);
    await checkAppUpdateViaClient();
    expect(storeSignals.appUpdateCheckError.value).toBeNull();
    expect(storeSignals.clientOfferedRelease.value).toBeNull();
  });
});

describe('startAppUpdateProgress', () => {
  it('subscribes once however many times a workspace remounts', () => {
    startAppUpdateProgress();
    startAppUpdateProgress();
    startAppUpdateProgress();
    expect(mocks.listen).toHaveBeenCalledTimes(1);
  });

  it('starts no timer of its own, because the gateway owns the check', () => {
    const setInterval = vi.spyOn(globalThis, 'setInterval');
    startAppUpdateProgress();
    expect(setInterval).not.toHaveBeenCalled();
  });

  it('stays a no-op outside the Tauri client', () => {
    mocks.isTauri.mockReturnValue(false);
    startAppUpdateProgress();
    expect(mocks.listen).not.toHaveBeenCalled();
  });
});

describe('refreshReleaseCheck', () => {
  it('records the gateway answer and offers the version it announces', async () => {
    mocks.requestUpdateCheck.mockResolvedValue(
      releaseCheckOf({ version: '9.1.0', install: 'desktop-app', command: null }),
    );
    await refreshReleaseCheck();
    expect(mocks.requestUpdateCheck).toHaveBeenCalledWith(false);
    expect(storeSignals.releaseCheck.value?.latest?.version).toBe('9.1.0');
    expect(lastToast().message).toBe('Lucidos 9.1.0 available');
  });

  it('makes the offer actionable in a Tauri client fronting a bundle', async () => {
    mocks.requestUpdateCheck.mockResolvedValue(
      releaseCheckOf({ version: '9.2.0', install: 'desktop-app', command: null }),
    );
    await refreshReleaseCheck();
    const opts = lastToastOpts() as { action: { label: string; onClick: () => void } };
    expect(opts.action.label).toBe('Update & restart');
    opts.action.onClick();
    expect(mocks.installAppUpdateAndRestart).toHaveBeenCalledTimes(1);
  });

  it('reads the attached relay before it words the offer in a browser session', async () => {
    mocks.isTauri.mockReturnValue(false);
    mocks.requestUpdateCheck.mockResolvedValue(releaseCheckOf({ version: '9.50.0', install: 'desktop-app' }));
    mocks.getGatewayStatus.mockResolvedValue({ build_id: 'b', update_available: false, update_relay: attachedClient() });
    await refreshReleaseCheck();
    const opts = lastToastOpts() as { action: { label: string } };
    expect(opts.action.label).toBe('Update desktop app');
  });

  it('routes a browser or PWA session instead of offering an install', async () => {
    mocks.isTauri.mockReturnValue(false);
    mocks.requestUpdateCheck.mockResolvedValue(
      releaseCheckOf({ version: '9.3.0', install: 'desktop-app', command: null }),
    );
    await refreshReleaseCheck();
    expect(lastToast().message).toBe('Lucidos 9.3.0 available');
    const opts = lastToastOpts() as { action: { label: string; onClick: () => void } };
    expect(opts.action.label).toBe('How to update');
    opts.action.onClick();
    expect(mocks.openSettingsSubview).toHaveBeenCalledWith('updates');
    expect(mocks.installAppUpdateAndRestart).not.toHaveBeenCalled();
  });

  it('refuses to read an unpollable gateway as up to date', async () => {
    mocks.requestUpdateCheck.mockResolvedValue(releaseCheckOf(null, { supported: false }));
    expect(await refreshReleaseCheck(true)).toEqual({
      kind: 'failed',
      reason: 'this install has no update check',
    });
  });

  it('routes a headless install to the page carrying its command', async () => {
    mocks.isTauri.mockReturnValue(false);
    mocks.requestUpdateCheck.mockResolvedValue(
      releaseCheckOf({
        version: '9.4.0',
        install: 'installer-rerun',
        command: 'curl -fsSL https://lucidos.dev/install.sh | sh -s -- --name default',
      }),
    );
    await refreshReleaseCheck();
    const opts = lastToastOpts() as { action: { label: string; onClick: () => void } };
    expect(opts.action.label).toBe('How to update');
    opts.action.onClick();
    expect(mocks.openSettingsSubview).toHaveBeenCalledWith('updates');
    expect(mocks.installAppUpdateAndRestart).not.toHaveBeenCalled();
  });

  it('offers one toast per version, however often it refreshes', async () => {
    mocks.requestUpdateCheck.mockResolvedValue(
      releaseCheckOf({ version: '9.5.0', install: 'desktop-app', command: null }),
    );
    await refreshReleaseCheck();
    await refreshReleaseCheck();
    await refreshReleaseCheck();
    expect(mocks.showToast).toHaveBeenCalledTimes(1);
  });

  it('says nothing when the gateway reports nothing published', async () => {
    mocks.requestUpdateCheck.mockResolvedValue(releaseCheckOf(null));
    await refreshReleaseCheck();
    expect(mocks.showToast).not.toHaveBeenCalled();
    expect(storeSignals.releaseCheck.value?.supported).toBe(true);
  });

  it('carries the offered release notes beside the version they describe', async () => {
    mocks.requestUpdateCheck.mockResolvedValue(
      releaseCheckOf({ version: '9.6.0', install: 'desktop-app', notes: '- a thing' }),
    );
    await refreshReleaseCheck();
    expect(storeSignals.latestTauriAppNotes.value).toBe('- a thing');
    const opts = lastToastOpts() as { secondaryAction: { onClick: () => void } };
    opts.secondaryAction.onClick();
    expect(mocks.openWhatsNew).toHaveBeenCalledWith('9.6.0');
  });

  it('offers no way to read notes the origin never carried', async () => {
    mocks.requestUpdateCheck.mockResolvedValue(
      releaseCheckOf({ version: '9.7.0', install: 'desktop-app' }),
    );
    await refreshReleaseCheck();
    expect(storeSignals.latestTauriAppNotes.value).toBeNull();
    expect(lastToastOpts().secondaryAction).toBeUndefined();
  });

  it('asks for a poll now when forced', async () => {
    await refreshReleaseCheck(true);
    expect(mocks.requestUpdateCheck).toHaveBeenCalledWith(true);
  });

  it('leaves the answer unknown when the gateway cannot answer', async () => {
    mocks.requestUpdateCheck.mockRejectedValue(new Error('404'));
    await refreshReleaseCheck();
    expect(storeSignals.releaseCheck.value).toBeNull();
    expect(storeSignals.appUpdateCheckError.value).toBeNull();
    expect(mocks.showToast).not.toHaveBeenCalled();
  });

  it('records the gateway poll failure rather than reporting no update', async () => {
    mocks.requestUpdateCheck.mockResolvedValue(
      releaseCheckOf(null, { last_error: 'origin served markup, not JSON' }),
    );
    await refreshReleaseCheck();
    expect(storeSignals.appUpdateCheckError.value).toContain('markup');
  });

  it('clears the recorded failure once a poll succeeds again', async () => {
    storeSignals.appUpdateCheckError.value = 'stale failure';
    mocks.requestUpdateCheck.mockResolvedValue(releaseCheckOf(null));
    await refreshReleaseCheck();
    expect(storeSignals.appUpdateCheckError.value).toBeNull();
  });

  it('records an unreachable gateway when the user asked', async () => {
    mocks.requestUpdateCheck.mockRejectedValue(new Error('connection refused'));
    await refreshReleaseCheck(true);
    expect(storeSignals.appUpdateCheckError.value).toContain('connection refused');
  });

  it('raises no offer on a phone', async () => {
    mocks.isTauri.mockReturnValue(false);
    mocks.thisDeviceIsMobile.mockReturnValue(true);
    mocks.requestUpdateCheck.mockResolvedValue(
      releaseCheckOf({ version: '9.9.1', install: 'desktop-app', notes: '- a thing' }),
    );
    expect(await refreshReleaseCheck()).toEqual({ kind: 'available', version: '9.9.1' });
    expect(mocks.showToast).not.toHaveBeenCalled();
  });

  it('still records the gateway answer on a phone', async () => {
    mocks.isTauri.mockReturnValue(false);
    mocks.thisDeviceIsMobile.mockReturnValue(true);
    mocks.requestUpdateCheck.mockResolvedValue(
      releaseCheckOf({ version: '9.9.2', install: 'desktop-app', notes: '- a thing' }),
    );
    await refreshReleaseCheck();
    expect(storeSignals.releaseCheck.value?.latest?.version).toBe('9.9.2');
    expect(storeSignals.latestTauriAppNotes.value).toBe('- a thing');
  });

  it('raises no offer over an install already running', async () => {
    storeSignals.appUpdateProgress.value = { version: '9.8.0', phase: 'installing' };
    mocks.requestUpdateCheck.mockResolvedValue(
      releaseCheckOf({ version: '9.8.0', install: 'desktop-app' }),
    );
    await refreshReleaseCheck();
    expect(mocks.showToast).not.toHaveBeenCalled();
    expect(storeSignals.releaseCheck.value?.latest?.version).toBe('9.8.0');
  });
});

describe('checkForUpdatesNow', () => {
  it('reports the release the gateway announced', async () => {
    storeSignals.releaseCheck.value = releaseCheckOf(null);
    mocks.requestUpdateCheck.mockResolvedValue(
      releaseCheckOf({ version: '9.20.0', install: 'desktop-app' }),
    );
    expect(await checkForUpdatesNow()).toEqual({ kind: 'available', version: '9.20.0' });
    expect(mocks.requestUpdateCheck).toHaveBeenCalledWith(true);
  });

  it('reports nothing newer as up to date', async () => {
    storeSignals.releaseCheck.value = releaseCheckOf(null);
    mocks.requestUpdateCheck.mockResolvedValue(releaseCheckOf(null));
    expect(await checkForUpdatesNow()).toEqual({ kind: 'up-to-date' });
  });

  it('answers from its own result, not from signals a later poll rewrites', async () => {
    storeSignals.releaseCheck.value = releaseCheckOf(null);
    mocks.requestUpdateCheck.mockResolvedValue(releaseCheckOf(null));
    const verdict = await checkForUpdatesNow();
    storeSignals.releaseCheck.value = releaseCheckOf({ version: '9.21.0' });
    expect(packagedUpdateVersion()).toBe('9.21.0');
    expect(verdict).toEqual({ kind: 'up-to-date' });
  });

  it('makes one request for two overlapping clicks, and answers both alike', async () => {
    storeSignals.releaseCheck.value = releaseCheckOf(null);
    mocks.requestUpdateCheck.mockResolvedValue(
      releaseCheckOf({ version: '9.22.0', install: 'desktop-app' }),
    );
    const [first, second] = await Promise.all([checkForUpdatesNow(), checkForUpdatesNow()]);
    expect(mocks.requestUpdateCheck).toHaveBeenCalledTimes(1);
    expect(first).toEqual(second);
  });

  it('raises the in-flight signal for the duration and lowers it after', async () => {
    storeSignals.releaseCheck.value = releaseCheckOf(null);
    let seen = false;
    mocks.requestUpdateCheck.mockImplementation(async () => {
      seen = storeSignals.appUpdateCheckInFlight.value;
      return releaseCheckOf(null);
    });
    await checkForUpdatesNow();
    expect(seen).toBe(true);
    expect(storeSignals.appUpdateCheckInFlight.value).toBe(false);
  });

  it('lowers the in-flight signal even when the check fails', async () => {
    storeSignals.releaseCheck.value = releaseCheckOf(null);
    mocks.requestUpdateCheck.mockRejectedValue(new Error('connection refused'));
    expect(await checkForUpdatesNow()).toEqual({
      kind: 'failed',
      reason: 'connection refused',
    });
    expect(storeSignals.appUpdateCheckInFlight.value).toBe(false);
  });

  it('reports a failed gateway poll as a failure, not as up to date', async () => {
    storeSignals.releaseCheck.value = releaseCheckOf(null);
    mocks.requestUpdateCheck.mockResolvedValue(
      releaseCheckOf(null, { last_error: 'origin served markup, not JSON' }),
    );
    expect(await checkForUpdatesNow()).toEqual({
      kind: 'failed',
      reason: 'origin served markup, not JSON',
    });
  });

  it('re-offers a release it has already offered this session', async () => {
    storeSignals.releaseCheck.value = releaseCheckOf(null);
    mocks.requestUpdateCheck.mockResolvedValue(
      releaseCheckOf({ version: '9.23.0', install: 'desktop-app' }),
    );
    await checkForUpdatesNow();
    mocks.showToast.mockClear();
    await checkForUpdatesNow();
    expect(lastToast().message).toBe('Lucidos 9.23.0 available');
  });

  it('falls back to the client updater when the gateway announces nothing', async () => {
    storeSignals.releaseCheck.value = null;
    mocks.checkAppUpdate.mockResolvedValue(offer('9.24.0'));
    expect(await checkForUpdatesNow()).toEqual({ kind: 'available', version: '9.24.0' });
    expect(mocks.requestUpdateCheck).not.toHaveBeenCalled();
  });

  it('refuses to guess in a session with no check to run', async () => {
    mocks.isTauri.mockReturnValue(false);
    storeSignals.releaseCheck.value = null;
    const verdict = await checkForUpdatesNow();
    expect(verdict.kind).toBe('failed');
  });

  it('defers to an install already running', async () => {
    storeSignals.appUpdateProgress.value = { version: '9.25.0', phase: 'downloading', downloaded: 1, total: null };
    expect(await checkForUpdatesNow()).toEqual({ kind: 'installing' });
    expect(mocks.requestUpdateCheck).not.toHaveBeenCalled();
  });

  it('never answers "up to date" while an older install serves this workspace', async () => {
    (globalThis as { __LUCIDOS_APP_RELEASE__?: string }).__LUCIDOS_APP_RELEASE__ = '0.36.0';
    storeSignals.lucidosRelease.value = '0.26.2';
    storeSignals.releaseCheck.value = null;

    const verdict = await checkForUpdatesNow();

    expect(verdict).toEqual({ kind: 'shadowed', engine: '0.26.2', client: '0.36.0' });
    expect(mocks.checkAppUpdate).not.toHaveBeenCalled();
  });

  it('still asks the client updater when the engine is current', async () => {
    (globalThis as { __LUCIDOS_APP_RELEASE__?: string }).__LUCIDOS_APP_RELEASE__ = '0.36.0';
    storeSignals.lucidosRelease.value = '0.36.0';
    storeSignals.releaseCheck.value = null;
    mocks.checkAppUpdate.mockResolvedValue(null);

    expect(await checkForUpdatesNow()).toEqual({ kind: 'up-to-date' });
    expect(mocks.checkAppUpdate).toHaveBeenCalled();
  });
});

describe('shadowedEngine', () => {
  it('reports only an engine OLDER than the app asking', () => {
    (globalThis as { __LUCIDOS_APP_RELEASE__?: string }).__LUCIDOS_APP_RELEASE__ = '0.36.0';

    storeSignals.lucidosRelease.value = '0.26.2';
    expect(shadowedEngine()).toEqual({ engine: '0.26.2', client: '0.36.0' });

    storeSignals.lucidosRelease.value = '99.0.0';
    expect(shadowedEngine()).toBeNull();

    storeSignals.lucidosRelease.value = '0.36.0';
    expect(shadowedEngine()).toBeNull();
  });

  it('compares the release, never the client build id', () => {
    (globalThis as { __LUCIDOS_APP_VERSION__?: string }).__LUCIDOS_APP_VERSION__ = '2026.10.03.0';
    (globalThis as { __LUCIDOS_APP_RELEASE__?: string }).__LUCIDOS_APP_RELEASE__ = '0.36.0';
    storeSignals.lucidosRelease.value = '0.36.0';
    expect(shadowedEngine()).toBeNull();
  });

  it('concludes nothing without both versions', () => {
    storeSignals.lucidosRelease.value = '0.26.2';
    expect(shadowedEngine()).toBeNull();

    (globalThis as { __LUCIDOS_APP_RELEASE__?: string }).__LUCIDOS_APP_RELEASE__ = '0.36.0';
    storeSignals.lucidosRelease.value = null;
    expect(shadowedEngine()).toBeNull();
  });
});

describe('reportUpdateCheck', () => {
  it('says so when there is nothing newer', () => {
    reportUpdateCheck({ kind: 'up-to-date' });
    expect(lastToast()).toMatchObject({ message: 'Lucidos is up to date', type: 'success' });
  });

  it('names the reason a check failed', () => {
    reportUpdateCheck({ kind: 'failed', reason: 'connection refused' });
    expect(lastToast()).toMatchObject({
      message: "Couldn't check for updates: connection refused",
      type: 'error',
    });
  });

  it('states the shadowed fact in exactly the current wording', () => {
    reportUpdateCheck({ kind: 'shadowed', engine: '0.26.2', client: '0.36.0' });
    expect(lastToast().message).toBe(
      'This workspace is served by Lucidos 0.26.2, but this app is 0.36.0. See System > Overview.',
    );
  });

  it('names both versions when another install is answering', () => {
    reportUpdateCheck({ kind: 'shadowed', engine: '0.26.2', client: '0.36.0' });
    const toast = lastToast();
    expect(toast.type).toBe('error');
    expect(toast.message).toContain('0.26.2');
    expect(toast.message).toContain('0.36.0');
    expect(toast.message).not.toContain('up to date');
  });

  it('stays quiet where a more specific surface already answered', () => {
    reportUpdateCheck({ kind: 'available', version: '9.30.0' });
    reportUpdateCheck({ kind: 'installing' });
    expect(mocks.showToast).not.toHaveBeenCalled();
  });
});

describe('updateControlLabel', () => {
  it('reports the check while one is running, whatever the control does', () => {
    for (const route of ROUTES) {
      expect(updateControlLabel(route, true), route).toBe('Checking…');
    }
  });

  it('owns every idle label rather than taking one from the caller', () => {
    expect(updateControlLabel('install', false)).toBe('Update & Restart');
    expect(updateControlLabel('check', false)).toBe('Check for Updates');
    expect(updateControlLabel('guide', false)).toBe('How to Update');
    expect(updateControlLabel('desktop', false)).toBe('How to Update');
  });

  it('names every route', () => {
    for (const route of ROUTES) {
      expect(updateControlLabel(route, false), route).not.toBe('');
    }
  });
});

describe('updateRoute', () => {
  it('installs here in a Tauri client fronting a bundle', () => {
    storeSignals.releaseCheck.value = releaseCheckOf({
      version: '9.40.0',
      install: 'desktop-app',
    });
    expect(updateRoute()).toBe('install');
  });

  it('routes a browser or PWA session to the page that can answer', () => {
    mocks.isTauri.mockReturnValue(false);
    storeSignals.releaseCheck.value = releaseCheckOf({
      version: '9.41.0',
      install: 'desktop-app',
    });
    expect(updateRoute()).toBe('guide');
  });

  it('routes a headless install to its installer command', () => {
    storeSignals.releaseCheck.value = releaseCheckOf({
      version: '9.42.0',
      install: 'installer-rerun',
      command: 'curl …',
    });
    expect(updateRoute()).toBe('guide');
  });

  it('offers a check when a release is known but none is offered', () => {
    storeSignals.releaseCheck.value = releaseCheckOf(null);
    expect(updateRoute()).toBe('check');
  });

  it('routes a source checkout to the page instead of a check it cannot run', () => {
    storeSignals.releaseCheck.value = releaseCheckOf(null, { supported: false });
    expect(updateRoute()).toBe('guide');
  });

  it('takes the word of a caller that already knows', () => {
    storeSignals.releaseCheck.value = releaseCheckOf(null);
    expect(updateRoute(true)).toBe('install');
  });

  it('never checks for a release the caller has already named', () => {
    mocks.isTauri.mockReturnValue(false);
    for (const check of [null, releaseCheckOf(null), releaseCheckOf(null, { supported: false })]) {
      storeSignals.releaseCheck.value = check;
      expect(updateRoute(true), `${check?.supported}`).toBe('guide');
    }
  });

  it('gives a phone one answer, whatever the state', () => {
    mocks.thisDeviceIsMobile.mockReturnValue(true);
    for (const offered of [true, false]) {
      for (const check of [null, releaseCheckOf(null), releaseCheckOf(null, { supported: false })]) {
        storeSignals.releaseCheck.value = check;
        expect(updateRoute(offered), `${offered}/${check?.supported}`).toBe('desktop');
      }
    }
  });

  it('always answers, whatever the session is', () => {
    for (const mobile of [true, false]) {
      for (const tauri of [true, false]) {
        for (const check of [null, releaseCheckOf(null), releaseCheckOf(null, { supported: false })]) {
          mocks.thisDeviceIsMobile.mockReturnValue(mobile);
          mocks.isTauri.mockReturnValue(tauri);
          storeSignals.releaseCheck.value = check;
          expect(ROUTES).toContain(updateRoute());
        }
      }
    }
  });
});

function attachedClient(version = '1.2.3', blocker: string | null = null): UpdateRelay {
  return { client: { version, blocker }, request: null };
}

describe('the relay route (ADR 0338)', () => {
  beforeEach(() => {
    mocks.isTauri.mockReturnValue(false);
    storeSignals.releaseCheck.value = releaseCheckOf({ version: '9.50.0', install: 'desktop-app' });
  });

  it('lets a phone ask the desktop app when one is attached and unblocked', () => {
    mocks.thisDeviceIsMobile.mockReturnValue(true);
    storeSignals.updateRelay.value = attachedClient();
    expect(updateRoute()).toBe('relay');
  });

  it('lets a desktop browser ask the same way', () => {
    storeSignals.updateRelay.value = attachedClient();
    expect(updateRoute()).toBe('relay');
  });

  it('keeps the old answer with no client, a blocked one, or an old gateway', () => {
    for (const desktop of [null, { client: null, request: null }, attachedClient('1.2.3', 'needs a password')]) {
      storeSignals.updateRelay.value = desktop;
      mocks.thisDeviceIsMobile.mockReturnValue(true);
      expect(updateRoute()).toBe('desktop');
      mocks.thisDeviceIsMobile.mockReturnValue(false);
      expect(updateRoute()).toBe('guide');
    }
  });

  it('offers no relay to a client already on the newest release', () => {
    storeSignals.updateRelay.value = attachedClient('9.50.0');
    expect(updateRoute()).toBe('guide');
  });

  it('offers the relay for a release the caller names and the gateway has not polled', () => {
    storeSignals.releaseCheck.value = releaseCheckOf(null);
    storeSignals.updateRelay.value = attachedClient();
    expect(updateRoute(true)).toBe('relay');
    mocks.thisDeviceIsMobile.mockReturnValue(true);
    expect(updateRoute(true)).toBe('relay');
  });

  it('lets a phone with a ready desktop app check when no release is known', () => {
    mocks.thisDeviceIsMobile.mockReturnValue(true);
    storeSignals.releaseCheck.value = releaseCheckOf(null);
    storeSignals.updateRelay.value = attachedClient();
    expect(updateRoute(false)).toBe('check');
  });

  it('keeps a phone on desktop when a check could not help', () => {
    mocks.thisDeviceIsMobile.mockReturnValue(true);
    for (const desktop of [null, { client: null, request: null }, attachedClient('1.2.3', 'needs a password')]) {
      storeSignals.releaseCheck.value = releaseCheckOf(null);
      storeSignals.updateRelay.value = desktop;
      expect(updateRoute(false)).toBe('desktop');
    }
    storeSignals.updateRelay.value = attachedClient();
    storeSignals.releaseCheck.value = releaseCheckOf(null, { supported: false });
    expect(updateRoute(false)).toBe('desktop');
  });

  it('never replaces an install the session can run itself', () => {
    mocks.isTauri.mockReturnValue(true);
    storeSignals.updateRelay.value = attachedClient();
    expect(updateRoute()).toBe('install');
  });

  it('labels the button for the machine it acts on', () => {
    expect(updateControlLabel('relay', false)).toBe('Update Desktop App');
  });

  it('asks the gateway, opens the dialog, and remembers the request', async () => {
    storeSignals.updateRelay.value = attachedClient();
    mocks.requestUpdateRelay.mockResolvedValue({ id: 'r-1', version: '9.50.0' });
    await followUpdateRoute('relay');
    expect(mocks.requestUpdateRelay).toHaveBeenCalledTimes(1);
    expect(storeSignals.appUpdateProgress.value).toEqual({ version: '9.50.0', phase: 'checking' });
    expect(storeSignals.relayedUpdate.value).toMatchObject({ id: 'r-1', version: '9.50.0', fromVersion: '1.2.3' });
  });

  it('says why when the gateway refuses', async () => {
    mocks.requestUpdateRelay.mockRejectedValue(new Error('the desktop app is not running on this machine'));
    await relayUpdateToDesktop();
    expect(lastToast()).toMatchObject({
      message: "Couldn't update the desktop app: the desktop app is not running on this machine",
      type: 'error',
    });
    expect(storeSignals.appUpdateProgress.value).toBeNull();
    expect(storeSignals.relayedUpdate.value).toBeNull();
  });

  it('asks once while a request is already being watched', async () => {
    storeSignals.relayedUpdate.value = { id: 'r-0', version: '9.50.0', fromVersion: '1.2.3', since: 0 };
    await relayUpdateToDesktop();
    expect(mocks.requestUpdateRelay).not.toHaveBeenCalled();
  });

  it('reads the relay state with the release check, for a session that cannot install', async () => {
    mocks.requestUpdateCheck.mockResolvedValue(releaseCheckOf({ version: '9.50.0', install: 'desktop-app' }));
    mocks.getGatewayStatus.mockResolvedValue({ build_id: 'b', update_available: false, update_relay: attachedClient() });
    await refreshReleaseCheck();
    expect(storeSignals.updateRelay.value).toEqual(attachedClient());
  });

  it('leaves the relay state alone in a client that installs itself', async () => {
    mocks.isTauri.mockReturnValue(true);
    await refreshReleaseCheck();
    expect(mocks.getGatewayStatus).not.toHaveBeenCalled();
  });
});

describe('canCheckForUpdatesHere', () => {
  it('follows the gateway when it has answered', () => {
    storeSignals.releaseCheck.value = releaseCheckOf(null);
    expect(canCheckForUpdatesHere()).toBe(true);
    storeSignals.releaseCheck.value = releaseCheckOf(null, { supported: false });
    expect(canCheckForUpdatesHere()).toBe(false);
  });

  it('falls back to the client updater when the gateway said nothing', () => {
    storeSignals.releaseCheck.value = null;
    expect(canCheckForUpdatesHere()).toBe(true);
    mocks.isTauri.mockReturnValue(false);
    expect(canCheckForUpdatesHere()).toBe(false);
  });
});

describe('followUpdateRoute', () => {
  it('installs on the install route', async () => {
    storeSignals.latestTauriAppVersion.value = '9.43.0';
    mocks.installAppUpdateAndRestart.mockResolvedValue(undefined);
    await followUpdateRoute('install');
    expect(mocks.installAppUpdateAndRestart).toHaveBeenCalledTimes(1);
  });

  it('checks and reports on the check route', async () => {
    storeSignals.releaseCheck.value = releaseCheckOf(null);
    mocks.requestUpdateCheck.mockResolvedValue(releaseCheckOf(null));
    await followUpdateRoute('check');
    expect(mocks.requestUpdateCheck).toHaveBeenCalledWith(true);
    expect(lastToast().message).toBe('Lucidos is up to date');
  });

  it('lands on the Maintenance section of Overview on the guide route', async () => {
    await followUpdateRoute('guide');
    expect(mocks.openSettingsSubview).toHaveBeenCalledWith('updates');
    expect(storeSignals.settingsScrollTarget.value).toBe('updates:updates');
    expect(mocks.installAppUpdateAndRestart).not.toHaveBeenCalled();
  });

  it('answers on a toast and navigates nowhere on the desktop route', async () => {
    await followUpdateRoute('desktop');
    expect(lastToast().message).toBe(
      'Update Lucidos on the machine that runs this workspace, not from this device.',
    );
    expect(mocks.openSettingsSubview).not.toHaveBeenCalled();
    expect(storeSignals.settingsScrollTarget.value).toBeNull();
    expect(mocks.installAppUpdateAndRestart).not.toHaveBeenCalled();
  });

  it('names no mechanism, whatever this workspace installs as', async () => {
    for (const install of ['desktop-app', 'installer-rerun', null] as const) {
      mocks.showToast.mockClear();
      storeSignals.releaseCheck.value = install
        ? releaseCheckOf({ version: '9.50.0', install })
        : releaseCheckOf(null, { supported: false });
      await followUpdateRoute('desktop');
      expect(lastToast().message, install ?? 'source checkout')
        .not.toMatch(/\b(app|installer|install\.sh|rebuild|pull)\b/i);
    }
  });

  it('replaces its own toast rather than stacking a second copy', async () => {
    await followUpdateRoute('desktop');
    await followUpdateRoute('desktop');
    const keys = mocks.showToast.mock.calls.map((c) => (c[2] as { key?: string })?.key);
    expect(keys).toEqual(['app-update-on-desktop', 'app-update-on-desktop']);
  });
});

describe('packagedUpdateVersion', () => {
  it.each([
    ['9.2.0-nightly.20261116.2', '9.2.0-nightly.20261116.10', true],
    ['9.2.0-nightly.20261116.1', '9.2.0', true],
    ['9.2.0-nightly.20261116.1', '9.1.0', false],
    ['9.2.0+a', '9.2.0+z', false],
  ])('orders client release offers %s -> %s', (current, offered, upgrade) => {
    storeSignals.releaseCheck.value = null;
    storeSignals.clientOfferedRelease.value = offered;
    storeSignals.latestTauriAppVersion.value = null;
    window.__LUCIDOS_APP_RELEASE__ = current;
    expect(packagedUpdateVersion()).toBe(upgrade ? offered : null);
  });

  it('prefers the gateway answer, which covers every install shape', () => {
    storeSignals.releaseCheck.value = releaseCheckOf({
      version: '9.1.0',
      install: 'installer-rerun',
      command: 'curl …',
    });
    storeSignals.latestTauriAppVersion.value = '0.27.0';
    expect(packagedUpdateVersion()).toBe('9.1.0');
  });

  it("falls back to the client updater's release while the gateway announces nothing", () => {
    storeSignals.releaseCheck.value = null;
    storeSignals.clientOfferedRelease.value = '1.3.0';
    window.__LUCIDOS_APP_VERSION__ = '2026.10.03.0';
    window.__LUCIDOS_APP_RELEASE__ = '1.2.3';
    expect(packagedUpdateVersion()).toBe('1.3.0');
  });

  it("falls back to the checkout's build id in a dev client", () => {
    storeSignals.releaseCheck.value = null;
    storeSignals.latestTauriAppVersion.value = '2026.10.03.2';
    window.__LUCIDOS_APP_VERSION__ = '2026.10.03.1';
    window.__LUCIDOS_APP_RELEASE__ = '1.2.3';
    expect(packagedUpdateVersion()).toBe('2026.10.03.2');
  });

  it('offers nothing when the gateway says the install is current', () => {
    storeSignals.releaseCheck.value = releaseCheckOf(null);
    expect(packagedUpdateVersion()).toBeNull();
  });
});

describe('canInstallUpdateHere', () => {
  it('is true for a Tauri client fronting a bundle', () => {
    storeSignals.releaseCheck.value = releaseCheckOf({
      version: '9.11.0',
      install: 'desktop-app',
    });
    expect(canInstallUpdateHere()).toBe(true);
  });

  it('is true for a Tauri client whose release names no install shape', () => {
    storeSignals.releaseCheck.value = releaseCheckOf({ version: '9.11.0', install: null });
    expect(canInstallUpdateHere()).toBe(true);
  });

  it('is false for a headless install, which re-runs the installer', () => {
    storeSignals.releaseCheck.value = releaseCheckOf({
      version: '9.11.0',
      install: 'installer-rerun',
      command: 'curl …',
    });
    expect(canInstallUpdateHere()).toBe(false);
  });

  it('is false in a browser or PWA session, which has no updater to call', () => {
    mocks.isTauri.mockReturnValue(false);
    storeSignals.releaseCheck.value = releaseCheckOf({
      version: '9.11.0',
      install: 'desktop-app',
    });
    expect(canInstallUpdateHere()).toBe(false);
  });

  it('is false when there is nothing on offer', () => {
    storeSignals.releaseCheck.value = releaseCheckOf(null);
    expect(canInstallUpdateHere()).toBe(false);
  });
});

describe('the check does not live in the webview', () => {
  const src = readFileSync(
    new URL('./app-update.ts', import.meta.url),
    'utf8',
  ) as string;

  it('starts no timer', () => {
    expect(src).not.toContain('setInterval');
    expect(src).not.toContain('setTimeout');
  });

  it('names no poll interval of its own', () => {
    expect(src).not.toContain('APP_UPDATE_POLL_MS');
  });

  it('reaches the release host only through the gateway', () => {
    expect(src).toContain('requestUpdateCheck(');
    expect(src).not.toContain('startAppUpdateChecks');
  });
});
