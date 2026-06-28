/**
 * Regression: the device push toggle is a controlled checkbox bound to
 * `device.push_enabled`, so the slider can't move until the `devices` signal
 * updates. `toggleDevicePush` used to flip that signal only after two mutating
 * round-trips AND a full `loadDevices()` re-fetch completed — so the user saw
 * the toggle sit unmoved (or snap back) for the whole network chain ("laggy").
 *
 * These tests pin the optimistic-update contract: the signal flips immediately,
 * and reverts if the network call fails.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

const setDevicePush = vi.fn();
const setPreference = vi.fn();
const listDevices = vi.fn();
const showToast = vi.fn();

vi.mock('../../api/client', () => ({
  registerDevice: vi.fn(),
  listDevices: (...args: unknown[]) => listDevices(...args),
  renameDevice: vi.fn(),
  setDevicePush: (...args: unknown[]) => setDevicePush(...args),
  deleteDevice: vi.fn(),
  setPreference: (...args: unknown[]) => setPreference(...args),
}));

vi.mock('../store', () => ({
  showToast: (...args: unknown[]) => showToast(...args),
  showConfirm: vi.fn(),
}));

function makeDevice(id: string, push_enabled: boolean) {
  return {
    id,
    name: null,
    user_agent: null,
    push_enabled,
    last_seen_at: '2026-06-26T00:00:00Z',
    created_at: '2026-06-26T00:00:00Z',
  };
}

/** A promise plus its resolve/reject handles, so a test can assert state
 *  *while* the mocked network call is still in flight. */
function deferred<T>() {
  let resolve!: (v: T) => void;
  let reject!: (e: unknown) => void;
  const promise = new Promise<T>((res, rej) => { resolve = res; reject = rej; });
  return { promise, resolve, reject };
}

describe('toggleDevicePush optimistic update', () => {
  beforeEach(() => {
    setDevicePush.mockReset();
    setPreference.mockReset();
    listDevices.mockReset();
    showToast.mockReset();
    listDevices.mockResolvedValue({ devices: [] });
    vi.resetModules();
  });
  afterEach(() => {
    vi.resetModules();
  });

  it('flips the signal before the network call resolves', async () => {
    const { devices, toggleDevicePush } = await import('./devices');
    devices.value = { status: 'loaded', data: [makeDevice('dev-a', false)] };

    const gate = deferred<void>();
    setDevicePush.mockReturnValue(gate.promise);
    setPreference.mockResolvedValue(undefined);

    const pending = toggleDevicePush('dev-a', true);

    // Optimistic: the signal already reflects the new value while the network
    // call is still unresolved — this is what makes the toggle feel instant.
    expect(devices.value.status).toBe('loaded');
    expect(devices.value.status === 'loaded' && devices.value.data[0].push_enabled).toBe(true);

    gate.resolve();
    await pending;
    expect(setDevicePush).toHaveBeenCalledWith('dev-a', true);
    expect(setPreference).toHaveBeenCalledWith('push_notifications', 'enabled', 'dev-a');
  });

  it('reverts the signal and toasts when the network call fails', async () => {
    const { devices, toggleDevicePush } = await import('./devices');
    devices.value = { status: 'loaded', data: [makeDevice('dev-a', true)] };

    setDevicePush.mockRejectedValue(new Error('boom'));
    setPreference.mockResolvedValue(undefined);

    await toggleDevicePush('dev-a', false);

    // Reverted back to the pre-toggle value, and the failure surfaced.
    expect(devices.value.status === 'loaded' && devices.value.data[0].push_enabled).toBe(true);
    expect(showToast).toHaveBeenCalledWith(
      expect.stringContaining('Failed to update push setting'),
      'error',
    );
  });

  it('leaves the toggle in its new state and reconciles on success', async () => {
    const { devices, toggleDevicePush } = await import('./devices');
    devices.value = { status: 'loaded', data: [makeDevice('dev-a', false)] };
    listDevices.mockResolvedValue({ devices: [makeDevice('dev-a', true)] });

    setDevicePush.mockResolvedValue(undefined);
    setPreference.mockResolvedValue(undefined);

    await toggleDevicePush('dev-a', true);

    expect(listDevices).toHaveBeenCalled();
    expect(devices.value.status === 'loaded' && devices.value.data[0].push_enabled).toBe(true);
    expect(showToast).not.toHaveBeenCalled();
  });
});
