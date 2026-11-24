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
    expect(activeProgressDialog.value.visible).toBe(false);
    const toast = toasts.value.find(t => t.key === RESTART_FAILURE_TOAST_KEY);
    expect(toast).toBeTruthy();
    expect(toast!.type).toBe('error');
    expect(toast!.message).toContain(reason);
  });

  it('dismisses the "New version available" switch toast so the dialog is the single surface', async () => {
    engineVersionReady.value = true;
    showToast('New version available.', 'info', {
      key: NEW_VERSION_TOAST_KEY,
      action: { label: 'Switch', onClick: () => {} },
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
    mockRestartEngine.mockResolvedValueOnce(undefined);

    await initiateEngineRestart();

    expect(engineRestarting.value).toBe(true);
    const dialog = activeProgressDialog.value;
    expect(dialog.visible).toBe(true);
    expect(dialog.title).toBe('Restarting engine');
    expect(dialog.progress).toBeNull();
    expect(dialog.cancel).toBeUndefined();
    expect(toasts.value).toHaveLength(0);
  });

  it('keeps restarting flag set on non-ApiError rejection (engine being killed)', async () => {
    mockRestartEngine.mockRejectedValueOnce(new TypeError('Failed to fetch'));

    await initiateEngineRestart();

    expect(engineRestarting.value).toBe(true);
    expect(activeProgressDialog.value.visible).toBe(true);
    expect(toasts.value.find(t => t.key === RESTART_FAILURE_TOAST_KEY)).toBeUndefined();
  });
});
