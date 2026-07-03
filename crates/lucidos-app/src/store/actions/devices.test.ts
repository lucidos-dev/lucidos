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

/**
 * Durable per-workspace device id (packaged desktop app). The id used to live only
 * in WKWebView localStorage, which a new DMG bundle re-buckets — so the app
 * registered a brand-new device on every update. `reconcileDeviceIdWithNativeStore`
 * adopts the id from the native store (which survives a reinstall) before the first
 * API call. The function is pure (deps injected), so these tests provide fakes.
 */
describe('reconcileDeviceIdWithNativeStore', () => {
  const KEY = 'lucidos-device-id';

  /** Minimal in-memory Storage stub with a spied setItem. */
  function makeStorage(init: Record<string, string> = {}) {
    const m = new Map(Object.entries(init));
    return {
      getItem: (k: string) => (m.has(k) ? (m.get(k) as string) : null),
      setItem: vi.fn((k: string, v: string) => {
        m.set(k, v);
      }),
      read: (k: string) => (m.has(k) ? (m.get(k) as string) : null),
    };
  }

  it('seeds localStorage from the native store after a reinstall (THE fix)', async () => {
    // Reinstall: WKWebView re-bucketed localStorage so it is empty, but the native
    // store survives and returns the OLD durable id. We must adopt it, not the fresh
    // candidate — otherwise a brand-new device registers.
    const { reconcileDeviceIdWithNativeStore } = await import('./devices');
    const storage = makeStorage();
    const randomUUID = vi.fn(() => 'fresh-uuid');
    const getOrCreate = vi.fn(async () => 'old-durable-id');

    await reconcileDeviceIdWithNativeStore({ workspace: 'alpha', getOrCreate, storage, randomUUID });

    expect(getOrCreate).toHaveBeenCalledWith('alpha', 'fresh-uuid');
    expect(storage.setItem).toHaveBeenCalledWith(KEY, 'old-durable-id');
    expect(storage.read(KEY)).toBe('old-durable-id');
  });

  it('does not churn an existing install (existing id is adopted as the candidate)', async () => {
    // First run of the fixed build with a pre-existing localStorage id: get-or-create
    // returns that same id, so nothing is rewritten and no random uuid is minted.
    const { reconcileDeviceIdWithNativeStore } = await import('./devices');
    const storage = makeStorage({ [KEY]: 'existing-id' });
    const randomUUID = vi.fn(() => 'fresh-uuid');
    const getOrCreate = vi.fn(async (_ws: string, candidate: string) => candidate);

    await reconcileDeviceIdWithNativeStore({ workspace: 'alpha', getOrCreate, storage, randomUUID });

    expect(getOrCreate).toHaveBeenCalledWith('alpha', 'existing-id');
    expect(randomUUID).not.toHaveBeenCalled();
    expect(storage.setItem).not.toHaveBeenCalled();
  });

  it('persists a freshly minted id on a true first run (empty native store)', async () => {
    const { reconcileDeviceIdWithNativeStore } = await import('./devices');
    const storage = makeStorage();
    const randomUUID = vi.fn(() => 'fresh-uuid');
    const getOrCreate = vi.fn(async (_ws: string, candidate: string) => candidate);

    await reconcileDeviceIdWithNativeStore({ workspace: 'alpha', getOrCreate, storage, randomUUID });

    expect(getOrCreate).toHaveBeenCalledWith('alpha', 'fresh-uuid');
    expect(storage.setItem).toHaveBeenCalledWith(KEY, 'fresh-uuid');
  });

  it('is best-effort: a failing native store never throws and never rewrites', async () => {
    const { reconcileDeviceIdWithNativeStore } = await import('./devices');
    const storage = makeStorage({ [KEY]: 'existing-id' });
    const randomUUID = vi.fn(() => 'fresh-uuid');
    const getOrCreate = vi.fn(async () => {
      throw new Error('IPC down');
    });

    await expect(
      reconcileDeviceIdWithNativeStore({ workspace: 'alpha', getOrCreate, storage, randomUUID }),
    ).resolves.toBeUndefined();
    expect(storage.setItem).not.toHaveBeenCalled();
    expect(storage.read(KEY)).toBe('existing-id');
  });

  it('seeds the candidate before the await so a concurrent getDeviceId stays consistent', async () => {
    // Empty storage (reinstall) + a native call that never resolves in time: the
    // candidate is seeded synchronously, so a getDeviceId() that runs mid-boot
    // returns this id rather than minting a different throwaway UUID (which would
    // register a spurious device — the timeout-edge race the seed closes).
    const { reconcileDeviceIdWithNativeStore } = await import('./devices');
    const storage = makeStorage();
    const randomUUID = vi.fn(() => 'fresh-uuid');
    let resolveIpc!: (v: string) => void;
    const getOrCreate = vi.fn(() => new Promise<string>((res) => { resolveIpc = res; }));

    const pending = reconcileDeviceIdWithNativeStore({
      workspace: 'alpha',
      getOrCreate,
      storage,
      randomUUID,
    });

    // Before the IPC resolves, storage already holds the candidate.
    expect(storage.read(KEY)).toBe('fresh-uuid');

    // The native store echoes the candidate (true first run) → no second write.
    resolveIpc('fresh-uuid');
    await pending;
    expect(storage.read(KEY)).toBe('fresh-uuid');
    expect(storage.setItem).toHaveBeenCalledTimes(1);
  });
});
