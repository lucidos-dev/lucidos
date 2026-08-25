import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
import type { AppUpdateProgress, AppUpdateRunning } from '../../utils/tauri';
import type { ReleaseCheck, ReleaseOffer } from '../../api/client/control';
// The real copy module, not a mock: these tests are about what the user reads,
// and it is pure, so there is nothing to stub.
import { appUpdateNarration, appUpdateDialogState } from '../progressDialogCopy';

const mocks = vi.hoisted(() => ({
  isTauri: vi.fn(() => true),
  checkAppUpdate: vi.fn(),
  installAppUpdateAndRestart: vi.fn(),
  cancelAppUpdate: vi.fn(),
  listen: vi.fn(),
  showToast: vi.fn(),
  removeToast: vi.fn(),
  openWhatsNew: vi.fn(),
  openSettingsSubview: vi.fn(),
  requestUpdateCheck: vi.fn(),
}));

/** What the gateway's `release_check` object looks like. Defaults describe an
 *  installed deployment with the notice acknowledged and nothing published. */
function releaseCheckOf(
  latest: Partial<ReleaseOffer> | null = null,
  over: Partial<ReleaseCheck> = {},
): ReleaseCheck {
  return {
    enabled: true,
    notice_acknowledged: true,
    supported: true,
    current_version: '1.2.3',
    checked_at: '2026-08-23T10:00:00Z',
    last_error: null,
    latest: latest
      ? { version: '9.9.9', notes: null, install: null, command: null, ...latest }
      : null,
    ...over,
  };
}

// The persistent Settings → System surface reads these, so the tests assert on
// the values that page would actually render. A `.value` box is all the action
// touches, and it avoids importing the real store (and its whole dependency
// graph) into a unit test.
const storeSignals = vi.hoisted(() => ({
  latestTauriAppVersion: { value: null as string | null },
  latestTauriAppNotes: { value: null as string | null },
  appUpdateCheckError: { value: null as string | null },
  appUpdateProgress: { value: null as AppUpdateProgress | null },
  releaseCheck: { value: null as ReleaseCheck | null },
}));

/** What `check_app_update` resolves to. Notes default to absent, which is the
 *  state every assertion below other than the What's-new ones describes. */
function offer(version: string, notes: string | null = null) {
  return { version, notes };
}

vi.mock('../../utils/platform', () => ({ isTauri: mocks.isTauri }));
// The offer toast's secondary action navigates; the navigation itself belongs to
// the menu action's own tests. Mocked rather than left real because pulling the
// real module in would drag the whole store graph through this file's partial
// `../store` mock.
vi.mock('./menu', () => ({
  openWhatsNew: mocks.openWhatsNew,
  openSettingsSubview: mocks.openSettingsSubview,
}));
vi.mock('../../api/client/control', () => ({ requestUpdateCheck: mocks.requestUpdateCheck }));
vi.mock('../../utils/tauri', () => ({
  checkAppUpdate: mocks.checkAppUpdate,
  installAppUpdateAndRestart: mocks.installAppUpdateAndRestart,
  cancelAppUpdate: mocks.cancelAppUpdate,
  listen: mocks.listen,
  APP_UPDATE_PROGRESS_EVENT: 'app-update-progress',
}));
vi.mock('../store', () => ({
  showToast: mocks.showToast,
  removeToast: mocks.removeToast,
  latestTauriAppVersion: storeSignals.latestTauriAppVersion,
  latestTauriAppNotes: storeSignals.latestTauriAppNotes,
  appUpdateCheckError: storeSignals.appUpdateCheckError,
  appUpdateProgress: storeSignals.appUpdateProgress,
  releaseCheck: storeSignals.releaseCheck,
}));

const {
  canInstallUpdateHere,
  checkAppUpdateViaClient,
  installAppUpdate,
  packagedUpdateVersion,
  refreshReleaseCheck,
  startAppUpdateProgress,
  stopAppUpdateProgress,
} = await import('./app-update');

/** Push a frame through the REAL subscription wiring — the handler is whatever
 *  `startAppUpdateProgress` registered, so these tests exercise the actual path
 *  an event takes rather than a private function called directly. */
let emitProgress: (frame: AppUpdateProgress) => void = () => {
  throw new Error('startAppUpdateProgress() must run before a frame can be emitted');
};

/** The options of the most recent `showToast` call. */
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
  storeSignals.latestTauriAppNotes.value = null;
  storeSignals.appUpdateCheckError.value = null;
  storeSignals.appUpdateProgress.value = null;
  storeSignals.releaseCheck.value = null;
});

afterEach(() => {
  // Drops the progress subscription, so the next test's
  // `startAppUpdateProgress` registers a fresh handler instead of being skipped
  // by the idempotence guard.
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

  // No `Content-Length` means there is no honest percentage. Inventing one (or
  // rendering NaN%) is the failure this pins.
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

  // Both the toast and the System page paint this fraction; an undercounted
  // Content-Length would otherwise run one of their bars past its own track.
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

  // The cancellation contract: an affordance is offered only while abandoning the
  // run is actually possible. Past the download there is no half-installed state
  // to return to, so claiming otherwise would be a lie.
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

// The run is narrated by the progress dialog, which is DERIVED from
// `appUpdateProgress` in store.ts. So what this module owes the user is the
// FRAME: record it and the dialog draws, clear it and the dialog closes. These
// assert the frame, then run it through the shipped builder to read what the
// dialog says. The derivation itself is pinned by
// store/__tests__/progress-dialog-source.test.ts.
describe('update progress narration', () => {
  const dialogNow = () => appUpdateDialogState(
    storeSignals.appUpdateProgress.value as AppUpdateRunning,
    () => {},
  );

  // The bug this whole surface exists to fix: the click used to produce nothing
  // visible until the update was over.
  it('narrates on the click rather than waiting for the first event', async () => {
    storeSignals.latestTauriAppVersion.value = '2026.7.30';
    // Read the frame from INSIDE the invoke: the dialog has to be up before the
    // IPC hop, which is the whole point. The resolve then ends the run.
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

  it('raises no toast for a running frame, and clears the offer behind it', () => {
    startAppUpdateProgress();
    emitProgress({ version: '2026.7.30', phase: 'downloading', downloaded: 50, total: 200 });
    expect(mocks.showToast).not.toHaveBeenCalled();
    // The offer would otherwise sit behind the modal, inviting a second click on
    // an update already running.
    expect(mocks.removeToast).toHaveBeenCalledWith('app-update-available');
  });

  it('drops the cancel affordance once the run has committed', () => {
    startAppUpdateProgress();
    emitProgress({ version: '2026.7.30', phase: 'installing' });
    const dialog = dialogNow();
    expect(dialog.cancel).toBeUndefined();
    expect(dialog.progress).toBeNull();
  });

  // A failure must end the run AND say why — this one runs on a click, so the
  // best-effort console.warn carve-out does not apply.
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

  // The install destroyed the app on disk without landing a replacement (F9),
  // which is NOT the same as an update that failed: there is nothing left to
  // retry against, and the message already carries the reinstall instruction.
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

  // A cancel re-offers the update because nothing was written; a destroyed
  // bundle must not, because clicking "Update & restart" again would download
  // and install into a location with no app in it.
  it('does not re-offer the update after a bundle-swap failure', () => {
    startAppUpdateProgress();
    emitProgress({ version: '2026.7.30', phase: 'installing' });
    emitProgress({ version: '2026.7.30', phase: 'bundle-swap-failed', message: 'gone' });
    expect(lastToast().opts.action).toBeUndefined();
  });

  // Nothing was written to disk, so the update is still there to install —
  // leaving the user with no affordance would strand them until the next poll.
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
    // One dialog, updated in place by the frame, and nothing stacked beside it.
    expect(storeSignals.appUpdateProgress.value).toEqual({ version: '2026.7.30', phase: 'installing' });
    expect(mocks.showToast).not.toHaveBeenCalled();
  });

  // A rejected invoke (ACL denial, dead bridge) is the one failure Rust can't
  // announce for itself.
  it('never leaves the dialog up when the invoke itself rejects', async () => {
    storeSignals.latestTauriAppVersion.value = '2026.7.30';
    mocks.installAppUpdateAndRestart.mockRejectedValue('ipc unavailable');
    await installAppUpdate();
    const { message, type } = lastToast();
    expect(type).toBe('error');
    expect(message).toContain('ipc unavailable');
    expect(storeSignals.appUpdateProgress.value).toBeNull();
  });

  // A poll firing mid-download would otherwise raise a stale "available" offer
  // behind the dialog narrating the run it is offering.
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

  it('clicking the toast action installs the update + restarts the stack', async () => {
    mocks.checkAppUpdate.mockResolvedValue(offer('2026.6.25'));
    mocks.installAppUpdateAndRestart.mockResolvedValue(undefined);
    await checkAppUpdateViaClient();
    const opts = mocks.showToast.mock.calls[0][2];
    opts.action.onClick();
    expect(mocks.installAppUpdateAndRestart).toHaveBeenCalledTimes(1);
  });

  // "What is in it?" is the question an update offer raises, and the manifest's
  // notes are the only thing that can answer it: the offered version postdates
  // this binary, so it is absent from the changelog baked into it.
  it("keeps the offered release's notes beside the version they describe", async () => {
    mocks.checkAppUpdate.mockResolvedValue(offer('2026.6.25', '### Added\n\n- a thing'));
    await checkAppUpdateViaClient();
    expect(storeSignals.latestTauriAppVersion.value).toBe('2026.6.25');
    expect(storeSignals.latestTauriAppNotes.value).toBe('### Added\n\n- a thing');
  });

  it('offers a way to read them, which opens the release it just announced', async () => {
    // Naming the version is the whole of it. An unnamed open falls back to
    // expanding the release already RUNNING, which is the one the offer is
    // asking the user to move off.
    mocks.checkAppUpdate.mockResolvedValue(offer('2026.6.25', '### Added\n\n- a thing'));
    await checkAppUpdateViaClient();
    const opts = mocks.showToast.mock.calls[0][2];
    expect(opts.secondaryAction.label).toBe("What's new");
    opts.secondaryAction.onClick();
    expect(mocks.openWhatsNew).toHaveBeenCalledWith('2026.6.25');
    // Reading is not taking: the primary action stays the only thing that
    // installs anything.
    expect(mocks.installAppUpdateAndRestart).not.toHaveBeenCalled();
  });

  it('re-offers after a cancel with the version still named', async () => {
    // The cancel path rebuilds the offer from the frame, and the link must not
    // quietly lose its release on the way through.
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
    // An affordance that opens onto nothing is worse than no affordance, and
    // falling back to the installed changelog would show the notes for the
    // version already running.
    mocks.checkAppUpdate.mockResolvedValue(offer('2026.6.25'));
    await checkAppUpdateViaClient();
    expect(mocks.showToast.mock.calls[0][2].secondaryAction).toBeUndefined();
  });

  it('drops the notes with the version when the update goes away', async () => {
    // A stale note beside a fresh version would tell the user what a DIFFERENT
    // update contains, so the two are written and cleared together.
    mocks.checkAppUpdate.mockResolvedValue(offer('2026.6.25', '### Added'));
    await checkAppUpdateViaClient();
    mocks.checkAppUpdate.mockResolvedValue(null);
    await checkAppUpdateViaClient();
    expect(storeSignals.latestTauriAppVersion.value).toBe(null);
    expect(storeSignals.latestTauriAppNotes.value).toBe(null);
  });

  it('swallows a failed check (best-effort) — no toast, retried next poll', async () => {
    mocks.checkAppUpdate.mockRejectedValue(new Error('network'));
    await checkAppUpdateViaClient();
    expect(mocks.showToast).not.toHaveBeenCalled();
  });

  // The toast is transient; Settings → System is the surface that persists, so
  // the outcome has to be RECORDED, not just announced.
  it('records the available version for the persistent System surface', async () => {
    mocks.checkAppUpdate.mockResolvedValue(offer('0.16.0'));
    await checkAppUpdateViaClient();
    expect(storeSignals.latestTauriAppVersion.value).toBe('0.16.0');
    expect(storeSignals.appUpdateCheckError.value).toBeNull();
  });

  // A silent failure is what made a stranded install indistinguishable from an
  // up-to-date one — the whole point of recording it.
  it('records why a check failed instead of only console.warn-ing', async () => {
    mocks.checkAppUpdate.mockRejectedValue(new Error('network'));
    await checkAppUpdateViaClient();
    expect(storeSignals.appUpdateCheckError.value).toContain('network');
    expect(mocks.showToast).not.toHaveBeenCalled();
  });

  // In a Tauri DEV client `check_app_update` is a no-op returning null, which is
  // indistinguishable from "up to date". Assigning that null blindly would wipe
  // the version connection.ts reads from the engine's /health — dev's only
  // source — and the two would fight on every poll.
  it('does not clobber a version it did not set', async () => {
    mocks.checkAppUpdate.mockResolvedValue(null);
    await checkAppUpdateViaClient(); // relinquish ownership if an earlier case took it
    storeSignals.latestTauriAppVersion.value = '2026.07.03.0'; // as if from /health
    await checkAppUpdateViaClient();
    expect(storeSignals.latestTauriAppVersion.value).toBe('2026.07.03.0');
  });

  it('does clear the version it set once the update is gone', async () => {
    mocks.checkAppUpdate.mockResolvedValue(offer('0.16.0'));
    await checkAppUpdateViaClient();
    expect(storeSignals.latestTauriAppVersion.value).toBe('0.16.0');

    mocks.checkAppUpdate.mockReset();
    mocks.checkAppUpdate.mockResolvedValue(null);
    await checkAppUpdateViaClient();
    expect(storeSignals.latestTauriAppVersion.value).toBeNull();
  });

  it('clears a previous error once a check succeeds again', async () => {
    mocks.checkAppUpdate.mockRejectedValue(new Error('network'));
    await checkAppUpdateViaClient();
    expect(storeSignals.appUpdateCheckError.value).not.toBeNull();

    mocks.checkAppUpdate.mockReset();
    mocks.checkAppUpdate.mockResolvedValue(null);
    await checkAppUpdateViaClient();
    expect(storeSignals.appUpdateCheckError.value).toBeNull();
    expect(storeSignals.latestTauriAppVersion.value).toBeNull();
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
  // The offer dedupe is per PROCESS, not per call, so each case announces its
  // own version. Reusing one across two cases would make the second silent for
  // the right reason and fail for the wrong one.
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

  // A browser or PWA session can install nothing, so an Update button there
  // would be a control that cannot do what it says.
  it('offers no install action in a browser or PWA session', async () => {
    mocks.isTauri.mockReturnValue(false);
    mocks.requestUpdateCheck.mockResolvedValue(
      releaseCheckOf({ version: '9.3.0', install: 'desktop-app', command: null }),
    );
    await refreshReleaseCheck();
    expect(lastToast().message).toBe('Lucidos 9.3.0 available');
    expect(lastToastOpts().action).toBeUndefined();
    expect(mocks.installAppUpdateAndRestart).not.toHaveBeenCalled();
  });

  // A headless install updates by re-running the installer, so the toast routes
  // to Settings, System, which is where the composed command is shown.
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
    expect(mocks.openSettingsSubview).toHaveBeenCalledWith('system');
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

  // The notes are what answer "what is in it", and they must describe the
  // version being offered rather than the one already installed.
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

  // An older gateway has no such route, and there is none at all on a direct
  // engine port. Both must read as "no offer", not as an error the user sees.
  it('leaves the answer unknown when the gateway cannot answer', async () => {
    mocks.requestUpdateCheck.mockRejectedValue(new Error('404'));
    await refreshReleaseCheck();
    expect(storeSignals.releaseCheck.value).toBeNull();
    expect(storeSignals.appUpdateCheckError.value).toBeNull();
    expect(mocks.showToast).not.toHaveBeenCalled();
  });

  // A poll that FAILED must never read as "you are up to date". Settings turns
  // this signal into a notice, and into the toast the Check button reports on.
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

  // The user clicked and is owed an answer. A forced call that cannot reach
  // the gateway says so, rather than falling through to "up to date".
  it('records an unreachable gateway when the user asked', async () => {
    mocks.requestUpdateCheck.mockRejectedValue(new Error('connection refused'));
    await refreshReleaseCheck(true);
    expect(storeSignals.appUpdateCheckError.value).toContain('connection refused');
  });

  // A run in flight owns the shared toast key, and its narration is a more
  // specific answer than a fresh offer would be.
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

describe('packagedUpdateVersion', () => {
  it('prefers the gateway answer, which covers every install shape', () => {
    storeSignals.releaseCheck.value = releaseCheckOf({
      version: '9.1.0',
      install: 'installer-rerun',
      command: 'curl …',
    });
    storeSignals.latestTauriAppVersion.value = '0.27.0';
    expect(packagedUpdateVersion()).toBe('9.1.0');
  });

  it('falls back to the client check while the gateway announces nothing', () => {
    storeSignals.releaseCheck.value = null;
    storeSignals.latestTauriAppVersion.value = '1.3.0';
    window.__LUCIDOS_APP_VERSION__ = '1.2.3';
    expect(packagedUpdateVersion()).toBe('1.3.0');
  });

  it('offers nothing when the gateway says the install is current', () => {
    storeSignals.releaseCheck.value = releaseCheckOf(null);
    expect(packagedUpdateVersion()).toBeNull();
  });
});

// An offer exists for every install shape, but only one session can act on it.
// Settings asks this before invoking the updater, so a "Check for Updates"
// click cannot reach Tauri IPC that is not there.
describe('canInstallUpdateHere', () => {
  it('is true for a Tauri client fronting a bundle', () => {
    storeSignals.releaseCheck.value = releaseCheckOf({
      version: '9.11.0',
      install: 'desktop-app',
    });
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

/**
 * The webview timer is gone, not dormant.
 *
 * ADR 0108 moved the check into the gateway for one reason: a timer here runs
 * once per open window, so N windows on one machine made N polls an hour. A
 * timer reintroduced beside the gateway's would put that straight back, and
 * nothing else in the suite would notice. It would simply ask twice.
 *
 * A source scan, because absence is the property. There is no call to make and
 * no state to observe, so only reading the module can prove it.
 */
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
    // `checkAppUpdate` is the Tauri fallback for a gateway too old to announce
    // anything, and it runs on user intent alone.
    expect(src).toContain('requestUpdateCheck(');
    expect(src).not.toContain('startAppUpdateChecks');
  });
});
