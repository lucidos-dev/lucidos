import { describe, it, expect, beforeEach, vi } from 'vitest';
import { engineRestarting, toasts, showToast, NEW_VERSION_TOAST_KEY, restartRequired, engineVersionReady, enginePackaged } from '../store';
import { ApiError } from '../../api/client';

const RESTART_TOAST_KEY = 'restart-required';

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
    const reason = 'Script not found: /Users/k/old-path/scripts/web-dev.sh';
    mockRestartEngine.mockRejectedValueOnce(new ApiError(500, reason));

    await initiateEngineRestart();

    expect(engineRestarting.value).toBe(false);
    const toast = toasts.value.find(t => t.key === RESTART_TOAST_KEY);
    expect(toast).toBeTruthy();
    expect(toast!.type).toBe('error');
    expect(toast!.message).toContain(reason);
  });

  it('dismisses the "New version available" switch toast so the progress toast is the single surface', async () => {
    // Regression: clicking "Switch to new version" stacked "Starting new version…"
    // on top of the still-visible "New version available." toast (two toasts at
    // once). The switch must replace that surface, not add to it.
    engineVersionReady.value = true; // a genuine new-version switch is available
    showToast('New version available.', 'info', {
      key: NEW_VERSION_TOAST_KEY,
      action: { label: 'Switch to new version', onClick: () => {} },
    });
    expect(toasts.value.some(t => t.key === NEW_VERSION_TOAST_KEY)).toBe(true);
    mockRestartEngine.mockResolvedValueOnce(undefined);

    await initiateEngineRestart();

    expect(toasts.value.some(t => t.key === NEW_VERSION_TOAST_KEY)).toBe(false);
    const progress = toasts.value.find(t => t.key === RESTART_TOAST_KEY);
    expect(progress!.message).toBe('Starting new version…');
  });

  it('a genuine switch (rebuilt binary ready) reads "Starting new version…"', async () => {
    engineVersionReady.value = true;
    mockRestartEngine.mockResolvedValueOnce(undefined);

    await initiateEngineRestart();

    expect(toasts.value.find(t => t.key === RESTART_TOAST_KEY)!.message).toBe('Starting new version…');
  });

  it('a plain restart (no new version) reads "Restarting engine…" and keeps the restarting flag set', async () => {
    // No pending change, no ready binary, engine not outdated → a plain respawn of
    // the running version. The progress toast must NOT claim a new version.
    mockRestartEngine.mockResolvedValueOnce(undefined);

    await initiateEngineRestart();

    expect(engineRestarting.value).toBe(true);
    const toast = toasts.value.find(t => t.key === RESTART_TOAST_KEY);
    expect(toast!.type).toBe('info');
    // A spinner signals ongoing work. It stays dismissible — the UI is no longer
    // deactivated during a restart, so the status banner is just a hint the user
    // can close.
    expect(toast!.message).toBe('Restarting engine…');
    expect(toast!.spinning).toBe(true);
    expect(toast!.dismissable).not.toBe(false);
  });

  // Network rejection after a successful 2xx: engine accepted the restart and
  // is now killing itself. ApiError-vs-TypeError is the signal that lets us
  // distinguish spawn failure from in-flight teardown.
  it('keeps restarting flag set on non-ApiError rejection (engine being killed)', async () => {
    mockRestartEngine.mockRejectedValueOnce(new TypeError('Failed to fetch'));

    await initiateEngineRestart();

    expect(engineRestarting.value).toBe(true);
    const toast = toasts.value.find(t => t.key === RESTART_TOAST_KEY);
    expect(toast!.type).toBe('info');
  });
});
