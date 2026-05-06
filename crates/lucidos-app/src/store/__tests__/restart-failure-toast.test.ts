import { describe, it, expect, beforeEach, vi } from 'vitest';
import { engineRestarting, toasts } from '../store';
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

  it('keeps restarting flag set when restart API succeeds', async () => {
    mockRestartEngine.mockResolvedValueOnce(undefined);

    await initiateEngineRestart();

    expect(engineRestarting.value).toBe(true);
    const toast = toasts.value.find(t => t.key === RESTART_TOAST_KEY);
    expect(toast!.type).toBe('info');
    expect(toast!.message).toBe('Restarting engine...');
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
