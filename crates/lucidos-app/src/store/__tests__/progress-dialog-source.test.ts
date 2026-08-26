import { describe, it, expect, beforeEach, vi } from 'vitest';

/**
 * `activeProgressDialog` is the one thing that decides whether a modal is over
 * the app, so these pin the two properties that keep it safe.
 *
 * It is DERIVED. A flow raises the dialog by setting the signal that says it is
 * running, and every existing clear of that signal closes it. Nothing writes
 * the dialog by hand, so no path can forget to take it down.
 *
 * And it is EXCLUSIVE. Two modals at once is not a state that exists, so a
 * precedence picks one rather than leaving it to whichever wrote last.
 *
 * The version below is a fixed synthetic value, deliberately not a real
 * release, so it can never collide with the `RELEASE` file being shipped.
 */

const mockCancelAppUpdate = vi.fn();
vi.mock('../../utils/tauri', async () => {
  const actual = await vi.importActual<typeof import('../../utils/tauri')>('../../utils/tauri');
  return { ...actual, cancelAppUpdate: () => mockCancelAppUpdate() };
});

const {
  activeProgressDialog,
  progressDialog,
  engineRestarting,
  engineRestartNewVersion,
  appUpdateProgress,
} = await import('../store');

beforeEach(() => {
  mockCancelAppUpdate.mockReset();
  engineRestarting.value = false;
  engineRestartNewVersion.value = false;
  appUpdateProgress.value = null;
  progressDialog.value = { visible: false, title: '', message: '', progress: null };
});

describe('activeProgressDialog', () => {
  it('shows nothing while neither flow is running', () => {
    expect(activeProgressDialog.value.visible).toBe(false);
  });

  it('follows the restart flag, in both of the restart wordings', () => {
    engineRestarting.value = true;
    expect(activeProgressDialog.value.title).toBe('Restarting engine');

    engineRestartNewVersion.value = true;
    expect(activeProgressDialog.value.title).toBe('Starting new version');
  });

  it('closes the instant the restart flag clears, wherever that happened', () => {
    // Reconnect, the safety timeout and a spawn failure all reach the dialog
    // this way, and none of them knows the dialog exists.
    engineRestarting.value = true;
    expect(activeProgressDialog.value.visible).toBe(true);

    engineRestarting.value = false;
    expect(activeProgressDialog.value.visible).toBe(false);
  });

  it('offers no way out of a restart, which cannot be called back', () => {
    engineRestarting.value = true;
    expect(activeProgressDialog.value.cancel).toBeUndefined();
    expect(activeProgressDialog.value.progress).toBeNull();
  });

  it('follows a live packaged-update frame, and closes when the run ends', () => {
    appUpdateProgress.value = { version: '9.9.9', phase: 'downloading', downloaded: 50, total: 200 };
    const dialog = activeProgressDialog.value;
    expect(dialog.title).toBe('Updating Lucidos');
    expect(dialog.progress).toBeCloseTo(0.25);

    appUpdateProgress.value = null;
    expect(activeProgressDialog.value.visible).toBe(false);
  });

  it('wires the update Cancel to the Rust command', () => {
    appUpdateProgress.value = { version: '9.9.9', phase: 'downloading', downloaded: 1, total: 2 };
    activeProgressDialog.value.cancel!.onClick();
    expect(mockCancelAppUpdate).toHaveBeenCalledTimes(1);
  });

  it('gives the restart precedence, so two flows cannot both claim the screen', () => {
    appUpdateProgress.value = { version: '9.9.9', phase: 'installing' };
    engineRestarting.value = true;
    expect(activeProgressDialog.value.title).toBe('Restarting engine');
  });

  it('falls back to the writable slot, which is what the gallery drives', () => {
    progressDialog.value = {
      visible: true,
      title: 'Updating Lucidos',
      message: 'Installing…',
      progress: null,
      cancel: { label: 'Close preview', onClick: () => {} },
    };
    expect(activeProgressDialog.value.cancel!.label).toBe('Close preview');

    // A real run still outranks the preview.
    engineRestarting.value = true;
    expect(activeProgressDialog.value.title).toBe('Restarting engine');
  });
});
