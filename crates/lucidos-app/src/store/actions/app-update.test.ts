import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import type { AppUpdateProgress, AppUpdateRunning } from '../../utils/tauri';

const mocks = vi.hoisted(() => ({
  isTauri: vi.fn(() => true),
  checkAppUpdate: vi.fn(),
  installAppUpdateAndRestart: vi.fn(),
  cancelAppUpdate: vi.fn(),
  listen: vi.fn(),
  showToast: vi.fn(),
  removeToast: vi.fn(),
  openSettingsSubview: vi.fn(),
}));

// The persistent Settings → System surface reads these, so the tests assert on
// the values that page would actually render. A `.value` box is all the action
// touches, and it avoids importing the real store (and its whole dependency
// graph) into a unit test.
const storeSignals = vi.hoisted(() => ({
  latestTauriAppVersion: { value: null as string | null },
  latestTauriAppNotes: { value: null as string | null },
  appUpdateCheckError: { value: null as string | null },
  appUpdateProgress: { value: null as AppUpdateProgress | null },
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
vi.mock('./menu', () => ({ openSettingsSubview: mocks.openSettingsSubview }));
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
}));

const {
  appUpdateNarration,
  checkForAppUpdate,
  installAppUpdate,
  recheckAppUpdateOnResume,
  startAppUpdateChecks,
  stopAppUpdateChecks,
} = await import('./app-update');

/** Mirrors `APP_UPDATE_RESUME_MIN_INTERVAL_MS` in the module under test. Kept as
 *  a literal rather than exported: the constant is an internal tuning knob, and a
 *  test that reads it back could not fail if it were changed by accident. */
const RESUME_THROTTLE_MS = 5 * 60 * 1000;

/** Push a frame through the REAL subscription wiring — the handler is whatever
 *  `startAppUpdateChecks` registered, so these tests exercise the actual path an
 *  event takes rather than a private function called directly. */
let emitProgress: (frame: AppUpdateProgress) => void = () => {
  throw new Error('startAppUpdateChecks() must run before a frame can be emitted');
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
  mocks.openSettingsSubview.mockReset();
  mocks.listen.mockReset();
  mocks.listen.mockImplementation((_event: string, handler: (e: { payload: AppUpdateProgress }) => void) => {
    emitProgress = (frame) => handler({ payload: frame });
    return Promise.resolve(() => {});
  });
  storeSignals.latestTauriAppVersion.value = null;
  storeSignals.latestTauriAppNotes.value = null;
  storeSignals.appUpdateCheckError.value = null;
  storeSignals.appUpdateProgress.value = null;
});

afterEach(() => {
  // Drops the interval AND the progress subscription, so the next test's
  // `startAppUpdateChecks` registers a fresh handler instead of being skipped by
  // the idempotence guard.
  stopAppUpdateChecks();
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

describe('update progress narration', () => {
  // The bug this whole surface exists to fix: the click used to produce nothing
  // visible until the update was over.
  it('narrates on the click rather than waiting for the first event', async () => {
    storeSignals.latestTauriAppVersion.value = '2026.7.30';
    mocks.installAppUpdateAndRestart.mockResolvedValue(undefined);
    await installAppUpdate();
    const first = mocks.showToast.mock.calls[0];
    expect(first[0]).toBe('Checking for updates…');
    expect((first[2] as Record<string, unknown>).spinning).toBe(true);
  });

  it('drives a spinning toast with a determinate bar while downloading', () => {
    startAppUpdateChecks();
    emitProgress({ version: '2026.7.30', phase: 'downloading', downloaded: 50, total: 200 });
    const { message, opts } = lastToast();
    expect(message).toContain('Downloading Lucidos 2026.7.30');
    expect(opts.spinning).toBe(true);
    expect(opts.progress).toBeCloseTo(0.25);
    // The gateway dies under the page in the last phases; without this opt-in the
    // narration would be suppressed by the disconnection it is explaining.
    expect(opts.showWhileUnavailable).toBe(true);
    expect((opts.secondaryAction as { label: string }).label).toBe('Cancel');
  });

  it('drops the cancel affordance once the run has committed', () => {
    startAppUpdateChecks();
    emitProgress({ version: '2026.7.30', phase: 'installing' });
    expect(lastToastOpts().secondaryAction).toBeUndefined();
    expect(lastToastOpts().progress).toBeNull();
  });

  it('routes the cancel button to the Rust command', () => {
    startAppUpdateChecks();
    emitProgress({ version: '2026.7.30', phase: 'downloading', downloaded: 1, total: 2 });
    (lastToastOpts().secondaryAction as { onClick: () => void }).onClick();
    expect(mocks.cancelAppUpdate).toHaveBeenCalledTimes(1);
  });

  // A failure must end the run AND say why — this one runs on a click, so the
  // best-effort console.warn carve-out does not apply.
  it('reports a failure with its reason and leaves nothing spinning', () => {
    startAppUpdateChecks();
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
    startAppUpdateChecks();
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
    startAppUpdateChecks();
    emitProgress({ version: '2026.7.30', phase: 'installing' });
    emitProgress({ version: '2026.7.30', phase: 'bundle-swap-failed', message: 'gone' });
    expect(lastToast().opts.action).toBeUndefined();
  });

  // Nothing was written to disk, so the update is still there to install —
  // leaving the user with no affordance would strand them until the next poll.
  it('re-offers the update after a cancel', () => {
    startAppUpdateChecks();
    emitProgress({ version: '2026.7.30', phase: 'downloading', downloaded: 1, total: 2 });
    emitProgress({ version: '2026.7.30', phase: 'cancelled' });
    const { message, opts } = lastToast();
    expect(message).toBe('Lucidos 2026.7.30 available');
    expect((opts.action as { label: string }).label).toBe('Update & restart');
    expect(storeSignals.appUpdateProgress.value).toBeNull();
  });

  it('keeps one toast for the whole run instead of stacking a new one per phase', () => {
    startAppUpdateChecks();
    emitProgress({ version: '2026.7.30', phase: 'checking' });
    emitProgress({ version: '2026.7.30', phase: 'downloading', downloaded: 1, total: 2 });
    emitProgress({ version: '2026.7.30', phase: 'installing' });
    const keys = new Set(mocks.showToast.mock.calls.map((c) => (c[2] as { key: string }).key));
    expect(keys).toEqual(new Set(['app-update-available']));
  });

  // A rejected invoke (ACL denial, dead bridge) is the one failure Rust can't
  // announce for itself.
  it('never leaves the toast spinning when the invoke itself rejects', async () => {
    storeSignals.latestTauriAppVersion.value = '2026.7.30';
    mocks.installAppUpdateAndRestart.mockRejectedValue('ipc unavailable');
    await installAppUpdate();
    const { message, type } = lastToast();
    expect(type).toBe('error');
    expect(message).toContain('ipc unavailable');
    expect(storeSignals.appUpdateProgress.value).toBeNull();
  });

  // Both surfaces share one key; a poll firing mid-download would otherwise
  // replace the live readout with a stale "available" offer.
  it('does not let the periodic check clobber a live run', async () => {
    storeSignals.appUpdateProgress.value = { version: '2026.7.30', phase: 'installing' };
    await checkForAppUpdate();
    expect(mocks.checkAppUpdate).not.toHaveBeenCalled();
    expect(mocks.showToast).not.toHaveBeenCalled();
  });

  it('subscribes once however many times the workspace remounts', () => {
    startAppUpdateChecks();
    startAppUpdateChecks();
    startAppUpdateChecks();
    expect(mocks.listen).toHaveBeenCalledTimes(1);
  });
});

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
    mocks.checkAppUpdate.mockResolvedValue(offer('2026.6.25'));
    await checkForAppUpdate();
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
    await checkForAppUpdate();
    const opts = mocks.showToast.mock.calls[0][2];
    opts.action.onClick();
    expect(mocks.installAppUpdateAndRestart).toHaveBeenCalledTimes(1);
  });

  // "What is in it?" is the question an update offer raises, and the manifest's
  // notes are the only thing that can answer it: the offered version postdates
  // this binary, so it is absent from the changelog baked into it.
  it("keeps the offered release's notes beside the version they describe", async () => {
    mocks.checkAppUpdate.mockResolvedValue(offer('2026.6.25', '### Added\n\n- a thing'));
    await checkForAppUpdate();
    expect(storeSignals.latestTauriAppVersion.value).toBe('2026.6.25');
    expect(storeSignals.latestTauriAppNotes.value).toBe('### Added\n\n- a thing');
  });

  it('offers a way to read them, which lands on What\'s New', async () => {
    mocks.checkAppUpdate.mockResolvedValue(offer('2026.6.25', '### Added\n\n- a thing'));
    await checkForAppUpdate();
    const opts = mocks.showToast.mock.calls[0][2];
    expect(opts.secondaryAction.label).toBe("What's new");
    opts.secondaryAction.onClick();
    expect(mocks.openSettingsSubview).toHaveBeenCalledWith('whats-new');
    // Reading is not taking: the primary action stays the only thing that
    // installs anything.
    expect(mocks.installAppUpdateAndRestart).not.toHaveBeenCalled();
  });

  it('offers no way to read notes the manifest never carried', async () => {
    // An affordance that opens onto nothing is worse than no affordance, and
    // falling back to the installed changelog would show the notes for the
    // version already running.
    mocks.checkAppUpdate.mockResolvedValue(offer('2026.6.25'));
    await checkForAppUpdate();
    expect(mocks.showToast.mock.calls[0][2].secondaryAction).toBeUndefined();
  });

  it('drops the notes with the version when the update goes away', async () => {
    // A stale note beside a fresh version would tell the user what a DIFFERENT
    // update contains, so the two are written and cleared together.
    mocks.checkAppUpdate.mockResolvedValue(offer('2026.6.25', '### Added'));
    await checkForAppUpdate();
    mocks.checkAppUpdate.mockResolvedValue(null);
    await checkForAppUpdate();
    expect(storeSignals.latestTauriAppVersion.value).toBe(null);
    expect(storeSignals.latestTauriAppNotes.value).toBe(null);
  });

  it('swallows a failed check (best-effort) — no toast, retried next poll', async () => {
    mocks.checkAppUpdate.mockRejectedValue(new Error('network'));
    await checkForAppUpdate();
    expect(mocks.showToast).not.toHaveBeenCalled();
  });

  // The toast is transient; Settings → System is the surface that persists, so
  // the outcome has to be RECORDED, not just announced.
  it('records the available version for the persistent System surface', async () => {
    mocks.checkAppUpdate.mockResolvedValue(offer('0.16.0'));
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
    mocks.checkAppUpdate.mockResolvedValue(offer('0.16.0'));
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

describe('recheckAppUpdateOnResume', () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  /** Put the module's "last checked" stamp at the current fake time, the way a
   *  startup or interval check would, so each case starts from a known baseline
   *  instead of inheriting whatever an earlier test left behind. */
  async function justChecked(): Promise<void> {
    mocks.checkAppUpdate.mockResolvedValue(null);
    await checkForAppUpdate();
    mocks.checkAppUpdate.mockClear();
    mocks.showToast.mockClear();
  }

  // The 2026-07-31 stranding, in miniature: a 0.18.0 client checked at launch
  // while 0.18.0 still WAS the latest, then sat there. Two newer releases were
  // published, and with no resume check and an hours-long interval the client
  // went on reporting itself current all morning.
  it('surfaces a release published while the client sat idle', async () => {
    await justChecked();
    mocks.checkAppUpdate.mockResolvedValue(offer('0.18.2'));
    vi.advanceTimersByTime(RESUME_THROTTLE_MS);
    await recheckAppUpdateOnResume();
    expect(mocks.checkAppUpdate).toHaveBeenCalledTimes(1);
    expect(lastToast().message).toBe('Lucidos 0.18.2 available');
  });

  // Window focus and visibilitychange fire on every alt-tab, and each check is a
  // network round-trip to the release host.
  it('collapses a flurry of window switches into no extra checks', async () => {
    await justChecked();
    vi.advanceTimersByTime(60 * 1000);
    await recheckAppUpdateOnResume();
    await recheckAppUpdateOnResume();
    await recheckAppUpdateOnResume();
    expect(mocks.checkAppUpdate).not.toHaveBeenCalled();
  });

  it('checks again once the throttle window has passed', async () => {
    await justChecked();
    vi.advanceTimersByTime(RESUME_THROTTLE_MS - 1);
    await recheckAppUpdateOnResume();
    expect(mocks.checkAppUpdate).not.toHaveBeenCalled();
    vi.advanceTimersByTime(1);
    await recheckAppUpdateOnResume();
    expect(mocks.checkAppUpdate).toHaveBeenCalledTimes(1);
  });

  // A resume that DID reach the host restarts the throttle; one that was
  // suppressed must not, or a single suppressed resume would push the next real
  // check out by another window every time the user switched away.
  it('restarts the throttle from the check, not from the attempt', async () => {
    await justChecked();
    vi.advanceTimersByTime(RESUME_THROTTLE_MS);
    await recheckAppUpdateOnResume();
    expect(mocks.checkAppUpdate).toHaveBeenCalledTimes(1);
    vi.advanceTimersByTime(RESUME_THROTTLE_MS - 1);
    await recheckAppUpdateOnResume();
    expect(mocks.checkAppUpdate).toHaveBeenCalledTimes(1);
  });

  // The suppression guard runs before the stamp, so a resume landing mid-install
  // must not count as a check and defer the next real one.
  it('does not consume the throttle when a run is already in flight', async () => {
    await justChecked();
    vi.advanceTimersByTime(RESUME_THROTTLE_MS);
    storeSignals.appUpdateProgress.value = { version: '0.18.2', phase: 'installing' };
    await recheckAppUpdateOnResume();
    expect(mocks.checkAppUpdate).not.toHaveBeenCalled();

    storeSignals.appUpdateProgress.value = null;
    await recheckAppUpdateOnResume();
    expect(mocks.checkAppUpdate).toHaveBeenCalledTimes(1);
  });

  it('stays a no-op outside the Tauri client', async () => {
    await justChecked();
    mocks.isTauri.mockReturnValue(false);
    vi.advanceTimersByTime(RESUME_THROTTLE_MS);
    await recheckAppUpdateOnResume();
    expect(mocks.checkAppUpdate).not.toHaveBeenCalled();
  });
});

describe('startAppUpdateChecks', () => {
  // The regression this exists to prevent: the old guard returned early when a
  // timer already existed, so only the FIRST workspace mount of a client process
  // ever checked. With an hours-long interval behind it, an update published mid-session
  // stayed invisible until the app was fully quit and relaunched.
  it('re-checks on every mount, not just the first of a client process', async () => {
    mocks.checkAppUpdate.mockResolvedValue(null);
    startAppUpdateChecks();
    startAppUpdateChecks();
    startAppUpdateChecks();
    expect(mocks.checkAppUpdate).toHaveBeenCalledTimes(3);
  });

  it('does not stack a second interval when called again', async () => {
    mocks.checkAppUpdate.mockResolvedValue(null);
    const setInterval = vi.spyOn(globalThis, 'setInterval');
    startAppUpdateChecks();
    startAppUpdateChecks();
    expect(setInterval).toHaveBeenCalledTimes(1);
  });

  it('stays a no-op outside the Tauri client', async () => {
    mocks.isTauri.mockReturnValue(false);
    startAppUpdateChecks();
    expect(mocks.checkAppUpdate).not.toHaveBeenCalled();
    expect(mocks.listen).not.toHaveBeenCalled();
  });
});
