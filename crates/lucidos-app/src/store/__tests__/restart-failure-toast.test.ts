import { describe, it, expect, beforeEach, vi } from 'vitest';
import { engineRestarting, engineRestartNewVersion, activeProgressDialog, toasts, showToast, NEW_VERSION_TOAST_KEY, restartRequired, engineVersionReady, enginePackaged } from '../store';
import { ApiError } from '../../api/client';

const RESTART_FAILURE_TOAST_KEY = 'restart-required';

const mockRestartEngine = vi.fn();
vi.mock('../../api/client', async () => {
  const actual = await vi.importActual<typeof import('../../api/client')>('../../api/client');
  return {
    ...actual,
    restartEngine: (...args: unknown[]) => mockRestartEngine(...args),
  };
});

const { initiateEngineRestart } = await import('../actions/chat-changes');

beforeEach(() => {
  engineRestarting.value = false;
  engineRestartNewVersion.value = false;
  toasts.value = [];
  // Default to a PLAIN restart (no new version) — individual tests opt into the
  // switch case by lighting engineVersionReady (dev) or enginePackaged+restartRequired.
  restartRequired.value = false;
  engineVersionReady.value = false;
  enginePackaged.value = false;
  mockRestartEngine.mockReset();
});

describe('initiateEngineRestart surfaces spawn failures', () => {
  it('shows error toast and clears restarting flag when restart API returns ApiError', async () => {
    const reason = 'Script not found: /Users/me/old-path/scripts/web-dev.sh';
    mockRestartEngine.mockRejectedValueOnce(new ApiError(500, reason));

    await initiateEngineRestart();

    expect(engineRestarting.value).toBe(false);
    // The dialog is derived from that flag, so the failure closes it. The error
    // must then be visible rather than trapped behind a modal.
    expect(activeProgressDialog.value.visible).toBe(false);
    const toast = toasts.value.find(t => t.key === RESTART_FAILURE_TOAST_KEY);
    expect(toast).toBeTruthy();
    expect(toast!.type).toBe('error');
    expect(toast!.message).toContain(reason);
  });

  it('dismisses the "New version available" switch toast so the dialog is the single surface', async () => {
    // Regression: clicking "Switch to new version" stacked the progress surface
    // on top of the still-visible "New version available." toast. The switch
    // must replace that surface, not add to it.
    engineVersionReady.value = true; // a genuine new-version switch is available
    showToast('New version available.', 'info', {
      key: NEW_VERSION_TOAST_KEY,
      action: { label: 'Switch to new version', onClick: () => {} },
    });
    expect(toasts.value.some(t => t.key === NEW_VERSION_TOAST_KEY)).toBe(true);
    mockRestartEngine.mockResolvedValueOnce(undefined);

    await initiateEngineRestart();

    expect(toasts.value.some(t => t.key === NEW_VERSION_TOAST_KEY)).toBe(false);
    expect(activeProgressDialog.value.title).toBe('Starting new version');
  });

  it('a genuine switch (rebuilt binary ready) reads "Starting new version"', async () => {
    engineVersionReady.value = true;
    mockRestartEngine.mockResolvedValueOnce(undefined);

    await initiateEngineRestart();

    expect(activeProgressDialog.value.title).toBe('Starting new version');
  });

  it('a plain restart (no new version) reads "Restarting engine" and keeps the flag set', async () => {
    // No pending change, no ready binary, engine not outdated: a plain respawn
    // of the running version. The dialog must NOT claim a new version.
    mockRestartEngine.mockResolvedValueOnce(undefined);

    await initiateEngineRestart();

    expect(engineRestarting.value).toBe(true);
    const dialog = activeProgressDialog.value;
    expect(dialog.visible).toBe(true);
    expect(dialog.title).toBe('Restarting engine');
    // Indeterminate, because a respawn has no honest percentage, and no way out
    // of it either.
    expect(dialog.progress).toBeNull();
    expect(dialog.cancel).toBeUndefined();
    // No toast narrates it any more.
    expect(toasts.value).toHaveLength(0);
  });

  // Network rejection after a successful 2xx: engine accepted the restart and
  // is now killing itself. ApiError-vs-TypeError is the signal that lets us
  // distinguish spawn failure from in-flight teardown.
  it('keeps restarting flag set on non-ApiError rejection (engine being killed)', async () => {
    mockRestartEngine.mockRejectedValueOnce(new TypeError('Failed to fetch'));

    await initiateEngineRestart();

    expect(engineRestarting.value).toBe(true);
    expect(activeProgressDialog.value.visible).toBe(true);
    expect(toasts.value.find(t => t.key === RESTART_FAILURE_TOAST_KEY)).toBeUndefined();
  });
});
