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
  handOverDevice: (...args: unknown[]) => handOverDeviceMock(...args),
  setPreference: (...args: unknown[]) => setPreference(...args),
}));

const pairingSession = vi.fn();
const handOverDeviceMock = vi.fn();
vi.mock('../../api/client/pairing', () => ({
  pairingSession: (...args: unknown[]) => pairingSession(...args),
}));

vi.mock('../store', () => ({
  showToast: (...args: unknown[]) => showToast(...args),
  showConfirm: vi.fn(),
}));

const postClientLog = vi.fn();
vi.mock('../../utils/clientLog', () => ({
  postClientLog: (...args: unknown[]) => postClientLog(...args),
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

/**
 * One device identity, minted at the gateway. The engine keyed its row on a
 * UUID this browser minted. The gateway keyed the pairing on one of its own.
 * So a single phone was two devices, with nothing joining them. Boot now
 * adopts the gateway's id and asks the engine to move the row onto it.
 *
 * Pure (deps injected), so these tests need no DOM and no gateway.
 */
describe('adoptGatewayDeviceIdentity', () => {
  const KEY = 'lucidos-device-id';

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

  it('adopts the gateway id and hands the old row over to it', async () => {
    const { adoptGatewayDeviceIdentity } = await import('./devices');
    const storage = makeStorage({ [KEY]: 'minted-locally' });
    const handOver = vi.fn().mockResolvedValue(undefined);

    const named = await adoptGatewayDeviceIdentity({
      session: async () => ({ device_id: 'paired-device' }),
      storage,
      handOver,
    });

    expect(named).toBe(true);
    expect(storage.read(KEY)).toBe('paired-device');
    expect(handOver).toHaveBeenCalledWith('minted-locally', 'paired-device');
  });

  it('leaves the localStorage id alone when no gateway answers', async () => {
    // A browser on a direct engine port. There is no paired-devices list there,
    // so there is no second identity for this one to be confused with.
    const { adoptGatewayDeviceIdentity } = await import('./devices');
    const storage = makeStorage({ [KEY]: 'minted-locally' });
    const handOver = vi.fn();

    const named = await adoptGatewayDeviceIdentity({
      session: async () => ({}),
      storage,
      handOver,
    });

    expect(named).toBe(false);
    expect(storage.read(KEY)).toBe('minted-locally');
    expect(storage.setItem).not.toHaveBeenCalled();
    expect(handOver).not.toHaveBeenCalled();
  });

  it('hands nothing over on a first run, having no old row to move', async () => {
    const { adoptGatewayDeviceIdentity } = await import('./devices');
    const storage = makeStorage();
    const handOver = vi.fn();

    await adoptGatewayDeviceIdentity({
      session: async () => ({ device_id: 'paired-device' }),
      storage,
      handOver,
    });

    expect(storage.read(KEY)).toBe('paired-device');
    expect(handOver).not.toHaveBeenCalled();
  });

  it('is a no-op on every load after the first', async () => {
    const { adoptGatewayDeviceIdentity } = await import('./devices');
    const storage = makeStorage({ [KEY]: 'paired-device' });
    const handOver = vi.fn();
    const previousId = vi.fn();

    await adoptGatewayDeviceIdentity({
      session: async () => ({ device_id: 'paired-device' }),
      storage,
      handOver,
      previousId,
    });

    expect(handOver).not.toHaveBeenCalled();
    expect(storage.setItem).not.toHaveBeenCalled();
    expect(previousId).not.toHaveBeenCalled();
  });

  it('recovers the id from the native store after a desktop reinstall', async () => {
    // A reinstall re-buckets the WKWebView container, so localStorage AND the
    // pairing cookie are gone. The window pairs again under a new name, and the
    // native store is the only place the old one survived.
    const { adoptGatewayDeviceIdentity } = await import('./devices');
    const storage = makeStorage();
    const handOver = vi.fn().mockResolvedValue(undefined);
    const remember = vi.fn().mockResolvedValue(undefined);

    await adoptGatewayDeviceIdentity({
      session: async () => ({ device_id: 'paired-again' }),
      storage,
      handOver,
      previousId: async () => 'before-reinstall',
      remember,
    });

    expect(handOver).toHaveBeenCalledWith('before-reinstall', 'paired-again');
    expect(storage.read(KEY)).toBe('paired-again');
    expect(remember).toHaveBeenCalledWith('paired-again');
  });

  it("prefers this webview's own last id over the native store's", async () => {
    const { adoptGatewayDeviceIdentity } = await import('./devices');
    const storage = makeStorage({ [KEY]: 'minted-locally' });
    const handOver = vi.fn().mockResolvedValue(undefined);
    const previousId = vi.fn().mockResolvedValue('stale-native');

    await adoptGatewayDeviceIdentity({
      session: async () => ({ device_id: 'paired-device' }),
      storage,
      handOver,
      previousId,
      remember: vi.fn().mockResolvedValue(undefined),
    });

    expect(handOver).toHaveBeenCalledWith('minted-locally', 'paired-device');
    expect(previousId).not.toHaveBeenCalled();
  });

  it('forgets nothing when the hand-over fails, so the next load retries', async () => {
    // THE failure that strands a row: commit the new id first and the only
    // reference to the old one is gone, with its push subscription and its
    // preferences under it. Nothing is written until the row has moved.
    const { adoptGatewayDeviceIdentity } = await import('./devices');
    const storage = makeStorage({ [KEY]: 'minted-locally' });
    const remember = vi.fn().mockResolvedValue(undefined);

    await expect(
      adoptGatewayDeviceIdentity({
        session: async () => ({ device_id: 'paired-device' }),
        storage,
        handOver: vi.fn().mockRejectedValue(new Error('engine said 500')),
        previousId: async () => null,
        remember,
      }),
    ).rejects.toThrow('engine said 500');

    expect(storage.read(KEY)).toBe('minted-locally');
    expect(storage.setItem).not.toHaveBeenCalled();
    expect(remember).not.toHaveBeenCalled();
  });

  it('keeps the native memory too when the hand-over fails after a reinstall', async () => {
    // Same rule on the path where localStorage is already empty: the native
    // store must still name the old row on the next attempt.
    const { adoptGatewayDeviceIdentity } = await import('./devices');
    const storage = makeStorage();
    const remember = vi.fn().mockResolvedValue(undefined);

    await expect(
      adoptGatewayDeviceIdentity({
        session: async () => ({ device_id: 'paired-again' }),
        storage,
        handOver: vi.fn().mockRejectedValue(new Error('engine said 500')),
        previousId: async () => 'before-reinstall',
        remember,
      }),
    ).rejects.toThrow('engine said 500');

    expect(storage.read(KEY)).toBe(null);
    expect(remember).not.toHaveBeenCalled();
  });

  it('swallows the failure at the production entry, and adopts nothing', async () => {
    // The caller is boot, so a rejection must not wedge it. It reports "no
    // gateway adopted us", which is true: the id is unchanged.
    const { adoptGatewayDeviceId } = await import('./devices');
    pairingSession.mockResolvedValue({ paired: true, device_id: 'paired-device', local: false });
    handOverDeviceMock.mockRejectedValue(new Error('engine said 500'));
    localStorage.setItem(KEY, 'minted-locally');

    await expect(adoptGatewayDeviceId(null)).resolves.toBe(false);
    expect(localStorage.getItem(KEY)).toBe('minted-locally');
  });

  it('leaves a breadcrumb naming both ids when the hand-over is refused', async () => {
    // Swallowing it is right, staying silent is not. A hand-over refused on
    // every load strands the migration, and the only symptom the user sees is
    // one device rendering as two rows. This puts it in `engine.log`.
    const { adoptGatewayDeviceId } = await import('./devices');
    postClientLog.mockClear();
    pairingSession.mockResolvedValue({ paired: true, device_id: 'paired-device', local: false });
    handOverDeviceMock.mockRejectedValue(new Error('400 cannot hand a row over to'));
    localStorage.setItem(KEY, 'minted-locally');

    await expect(adoptGatewayDeviceId(null)).resolves.toBe(false);

    expect(postClientLog).toHaveBeenCalledWith(
      'devices',
      'hand_over_failed',
      expect.objectContaining({
        old_device_id: 'minted-locally',
        device_id: 'paired-device',
        reason: expect.stringContaining('cannot hand a row over to'),
      }),
    );
  });

  it('leaves no breadcrumb when the hand-over lands', async () => {
    const { adoptGatewayDeviceId } = await import('./devices');
    postClientLog.mockClear();
    pairingSession.mockResolvedValue({ paired: true, device_id: 'paired-device', local: false });
    handOverDeviceMock.mockResolvedValue({ success: true, outcome: 'moved' });
    localStorage.setItem(KEY, 'minted-locally');

    await expect(adoptGatewayDeviceId(null)).resolves.toBe(true);
    expect(postClientLog).not.toHaveBeenCalled();
  });
});

/**
 * Source scan: the adoption must be awaited in boot, before anything registers
 * a device. `registerCurrentDevice` upserts the row, and the engine refuses a
 * hand-over once the new id has one, so the wrong order does not fail loudly.
 * It just means the migration silently never runs.
 */
describe('boot order', () => {
  it('awaits the gateway adoption in main.tsx, and registers nowhere near it', async () => {
    // @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node
    const { readFileSync } = await import('node:fs');
    const main = readFileSync(
      new URL('../../main.tsx', import.meta.url),
      'utf8',
    );
    expect(main).toContain('await adoptGatewayDeviceId(');
    expect(main).not.toContain('registerCurrentDevice');
  });
});
