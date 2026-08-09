/**
 * `setDevicePushEnabled` is the one switch behind both push rows (Settings →
 * Devices, and Appearance & Behavior → Notifications), plus the convenience it
 * carries: having turned push on for the phone in your hand, it offers to turn
 * it off on your OTHER phones, so one notification stops buzzing three devices.
 *
 * The offer is the part with teeth, because saying yes mutates devices the user
 * is not holding. These tests pin all four of its edges: it fires only from a
 * mobile device, only about mobile ones, only when there is something to
 * silence, and it changes nothing on cancel.
 *
 * Wired against the real `devices.ts` with a fake server behind
 * `api/client`, rather than mocking the store actions: the thing worth pinning
 * is which devices actually get a `setDevicePush(id, false)`, and a mocked
 * action would only prove the call was forwarded.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { DeviceInfo } from '../../api/types';

const setDevicePush = vi.fn();
const setPreference = vi.fn();
const listDevices = vi.fn();
const showToast = vi.fn();
const showConfirm = vi.fn();

// A Vitest run IS a Vite dev-server bundle, so the live `isDevServerBundle()`
// reports true and `initPushSubscription` would short-circuit on the frontend
// preview's no-service-worker gate before reaching anything below. Same pin as
// `push.test.ts`: these tests exercise the PRODUCTION path, and the gate itself
// is covered by `utils/devServerBundle.test.ts` and the pure
// `pushUnsupportedReason` cases.
vi.mock('../../utils/devServerBundle', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../utils/devServerBundle')>()),
  isDevServerBundle: () => false,
}));

vi.mock('../../api/client', () => ({
  API: '/api/v1',
  json: async () => ({ public_key: 'BPk-test-vapid-key' }),
  mutatingFetch: async () => new Response('{}', { status: 200 }),
  throwIfNotOk: async () => {},
  registerDevice: vi.fn(),
  listDevices: (...args: unknown[]) => listDevices(...args),
  renameDevice: vi.fn(),
  setDevicePush: (...args: unknown[]) => setDevicePush(...args),
  deleteDevice: vi.fn(),
  setPreference: (...args: unknown[]) => setPreference(...args),
}));

vi.mock('../store', () => ({
  showToast: (...args: unknown[]) => showToast(...args),
  showConfirm: (...args: unknown[]) => showConfirm(...args),
}));

const UA = {
  iphone:
    'Mozilla/5.0 (iPhone; CPU iPhone OS 18_5 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.5 Mobile/15E148 Safari/604.1',
  android:
    'Mozilla/5.0 (Linux; Android 15; Pixel 9) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0.0.0 Mobile Safari/537.36',
  mac:
    'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0.0.0 Safari/537.36',
} as const;

const THIS_DEVICE = 'dev-this-phone';

function makeDevice(id: string, userAgent: string, pushEnabled: boolean): DeviceInfo {
  return {
    id,
    name: null,
    user_agent: userAgent,
    push_enabled: pushEnabled,
    last_seen_at: '2026-08-08T00:00:00Z',
    created_at: '2026-08-08T00:00:00Z',
  };
}

/** A fake engine holding the device list, so `setDevicePush` writes are visible
 *  to the `loadDevices()` refetch that follows them. That refetch is what the
 *  offer reads to decide whether there is anything left to silence. */
function installFakeServer(initial: DeviceInfo[]) {
  const rows = initial.map((d) => ({ ...d }));
  setDevicePush.mockImplementation(async (id: string, enabled: boolean) => {
    const row = rows.find((d) => d.id === id);
    if (row) row.push_enabled = enabled;
    return { success: true };
  });
  setPreference.mockResolvedValue({ success: true });
  listDevices.mockImplementation(async () => ({ devices: rows.map((d) => ({ ...d })) }));
  return rows;
}

/** The browser side of enabling push: a granted permission and a service worker
 *  that hands back a subscription. `initPushSubscription` must clear all of this
 *  before the device flag is ever written. */
function installBrowserPushStubs(permission: NotificationPermission = 'granted') {
  const registration = {
    pushManager: {
      getSubscription: async () => null,
      subscribe: async () => ({
        toJSON: () => ({ endpoint: 'https://push/x', keys: { p256dh: 'p', auth: 'a' } }),
      }),
    },
  };
  Object.defineProperty(globalThis.navigator, 'serviceWorker', {
    value: { register: async () => registration, ready: Promise.resolve(registration) },
    configurable: true,
  });
  (globalThis as unknown as { Notification: typeof Notification }).Notification = Object.assign(
    function () { /* stub */ } as unknown as typeof Notification,
    { permission, requestPermission: async () => permission },
  ) as typeof Notification;
  (globalThis as unknown as { PushManager: object }).PushManager = function () { /* stub */ };
  Object.defineProperty(window, 'isSecureContext', { value: true, configurable: true });
}

/** `platform.ts` reads the user-agent once at module load, so this has to be set
 *  before the `await import('./push')` in each test (`vi.resetModules()` in
 *  `beforeEach` is what makes the re-read happen).
 *
 *  Standalone rides along because `initPushSubscription` refuses a non-installed
 *  iOS page outright ("add Lucidos to your home screen first"), which is a real
 *  guard but not the one under test here: the device is an installed PWA. */
function setUserAgent(ua: string) {
  Object.defineProperty(globalThis.navigator, 'userAgent', { value: ua, configurable: true });
  Object.defineProperty(globalThis.navigator, 'standalone', { value: true, configurable: true });
}

/** Every device `setDevicePush` was told to switch OFF. */
function switchedOff(): string[] {
  return setDevicePush.mock.calls.filter((c) => c[1] === false).map((c) => c[0] as string);
}

describe('setDevicePushEnabled', () => {
  beforeEach(() => {
    setDevicePush.mockReset();
    setPreference.mockReset();
    listDevices.mockReset();
    showToast.mockReset();
    showConfirm.mockReset();
    showConfirm.mockResolvedValue(false);
    localStorage.setItem('lucidos-device-id', THIS_DEVICE);
    // The suite runs on the node environment with hand-rolled browser stubs
    // (`src/test-setup.ts`), so there is no `window.location` to read the
    // service-worker scope origin from until one is declared.
    Object.defineProperty(window, 'location', {
      value: { origin: 'https://lucidos.test' },
      configurable: true,
    });
    installBrowserPushStubs();
    setUserAgent(UA.iphone);
    vi.resetModules();
  });
  afterEach(() => {
    localStorage.removeItem('lucidos-device-id');
    vi.resetModules();
  });

  it('turns push off on the other phones once the user confirms', async () => {
    const rows = installFakeServer([
      makeDevice(THIS_DEVICE, UA.iphone, false),
      makeDevice('dev-old-phone', UA.iphone, true),
      makeDevice('dev-tablet', UA.android, true),
      makeDevice('dev-laptop', UA.mac, true),
    ]);
    showConfirm.mockResolvedValue(true);

    const { setDevicePushEnabled } = await import('./push');
    await setDevicePushEnabled(THIS_DEVICE, true);

    expect(showConfirm).toHaveBeenCalledTimes(1);
    // The laptop is a complementary surface, not a duplicate: it keeps its push.
    expect(switchedOff().sort()).toEqual(['dev-old-phone', 'dev-tablet']);
    expect(rows.find((d) => d.id === 'dev-laptop')!.push_enabled).toBe(true);
    expect(rows.find((d) => d.id === THIS_DEVICE)!.push_enabled).toBe(true);
    expect(setPreference).toHaveBeenCalledWith('push_notifications', 'declined', 'dev-old-phone');
  });

  it('reloads only after every disable has settled, so a partial failure cannot be read mid-batch', async () => {
    // With `Promise.all`, the first rejection resumes the caller while the other
    // writes are still in flight, and the reload then reconciles a device back
    // to "on" that goes off a moment later.
    const rows = installFakeServer([
      makeDevice(THIS_DEVICE, UA.iphone, false),
      makeDevice('dev-fails', UA.iphone, true),
      makeDevice('dev-slow', UA.android, true),
    ]);
    let releaseSlow!: () => void;
    const slow = new Promise<void>((res) => { releaseSlow = () => res(); });
    setDevicePush.mockImplementation(async (id: string, enabled: boolean) => {
      if (id === 'dev-fails' && !enabled) throw new Error('engine down');
      if (id === 'dev-slow') await slow;
      const row = rows.find((d) => d.id === id);
      if (row) row.push_enabled = enabled;
      return { success: true };
    });
    listDevices.mockImplementation(async () => ({ devices: rows.map((d) => ({ ...d })) }));
    showConfirm.mockResolvedValue(true);

    const { setDevicePushEnabled } = await import('./push');
    const { devices } = await import('./devices');
    const pending = setDevicePushEnabled(THIS_DEVICE, true);
    // Let the fast failure land, then release the straggler. If the reload ran
    // on the rejection it has already happened, with dev-slow still on.
    await Promise.resolve();
    releaseSlow();
    await pending;

    expect(devices.value.status).toBe('loaded');
    const slowRow = devices.value.status === 'loaded'
      ? devices.value.data.find((d) => d.id === 'dev-slow')
      : undefined;
    expect(slowRow?.push_enabled, 'the reload must see the slow write').toBe(false);
    expect(showToast).toHaveBeenCalledWith(
      expect.stringContaining('Failed to turn push off'),
      'error',
    );
  });

  it('changes nothing on the other devices when the user declines', async () => {
    const rows = installFakeServer([
      makeDevice(THIS_DEVICE, UA.iphone, false),
      makeDevice('dev-old-phone', UA.iphone, true),
    ]);
    showConfirm.mockResolvedValue(false);

    const { setDevicePushEnabled } = await import('./push');
    await setDevicePushEnabled(THIS_DEVICE, true);

    expect(showConfirm).toHaveBeenCalledTimes(1);
    expect(switchedOff()).toEqual([]);
    expect(rows.find((d) => d.id === 'dev-old-phone')!.push_enabled).toBe(true);
  });

  it('does not offer to silence the others when this device failed to switch on', async () => {
    // `toggleDevicePush` swallows a failed write into a toast and returns
    // normally, so nothing downstream can tell the enable did not land. Without
    // the confirmed-on check, the offer still fires, and confirming it leaves
    // the user with no device getting a push at all.
    installFakeServer([
      makeDevice(THIS_DEVICE, UA.iphone, false),
      makeDevice('dev-old-phone', UA.iphone, true),
    ]);
    setDevicePush.mockRejectedValue(new Error('engine down'));

    const { setDevicePushEnabled } = await import('./push');
    const { devices } = await import('./devices');
    devices.value = {
      status: 'loaded',
      data: [
        makeDevice(THIS_DEVICE, UA.iphone, false),
        makeDevice('dev-old-phone', UA.iphone, true),
      ],
    };

    await setDevicePushEnabled(THIS_DEVICE, true);

    expect(showConfirm).not.toHaveBeenCalled();
    expect(showToast).toHaveBeenCalledWith(
      expect.stringContaining('Failed to update push setting'),
      'error',
    );
  });

  it('never offers from a desktop, where a phone is a second surface rather than a duplicate', async () => {
    setUserAgent(UA.mac);
    installFakeServer([
      makeDevice(THIS_DEVICE, UA.mac, false),
      makeDevice('dev-old-phone', UA.iphone, true),
    ]);

    const { setDevicePushEnabled } = await import('./push');
    await setDevicePushEnabled(THIS_DEVICE, true);

    expect(showConfirm).not.toHaveBeenCalled();
    expect(switchedOff()).toEqual([]);
  });

  it('stays quiet when no other mobile device has push on', async () => {
    installFakeServer([
      makeDevice(THIS_DEVICE, UA.iphone, false),
      makeDevice('dev-old-phone', UA.iphone, false),
      makeDevice('dev-laptop', UA.mac, true),
    ]);

    const { setDevicePushEnabled } = await import('./push');
    await setDevicePushEnabled(THIS_DEVICE, true);

    expect(showConfirm).not.toHaveBeenCalled();
  });

  it('stays quiet when turning push OFF', async () => {
    installFakeServer([
      makeDevice(THIS_DEVICE, UA.iphone, true),
      makeDevice('dev-old-phone', UA.iphone, true),
    ]);

    const { setDevicePushEnabled } = await import('./push');
    await setDevicePushEnabled(THIS_DEVICE, false);

    expect(showConfirm).not.toHaveBeenCalled();
    expect(switchedOff()).toEqual([THIS_DEVICE]);
  });

  it('writes no device flag at all when the browser refuses permission', async () => {
    // The flag alone would leave the engine pushing to an endpoint that does not
    // exist, and the row would claim an "on" the OS will never honour.
    installBrowserPushStubs('denied');
    installFakeServer([makeDevice(THIS_DEVICE, UA.iphone, false)]);

    const { setDevicePushEnabled } = await import('./push');
    await setDevicePushEnabled(THIS_DEVICE, true);

    expect(setDevicePush).not.toHaveBeenCalled();
    expect(showToast).toHaveBeenCalledWith(expect.stringContaining('denied'), 'error');
  });

  it('does not offer when push is turned on for a device other than this one', async () => {
    // Enabling for another device cannot actually work (only that browser can
    // subscribe itself), but the offer is about the device in the user's hand
    // and must not fire for another id whatever the caller passes.
    installFakeServer([
      makeDevice(THIS_DEVICE, UA.iphone, false),
      makeDevice('dev-old-phone', UA.iphone, true),
      makeDevice('dev-other', UA.iphone, false),
    ]);

    const { setDevicePushEnabled } = await import('./push');
    await setDevicePushEnabled('dev-other', true);

    expect(showConfirm).not.toHaveBeenCalled();
  });
});

describe('otherPushEnabledMobileDevices', () => {
  it('keeps only other devices that are mobile and still pushing', async () => {
    const { otherPushEnabledMobileDevices } = await import('./push');
    const all = [
      makeDevice(THIS_DEVICE, UA.iphone, true),
      makeDevice('dev-old-phone', UA.iphone, true),
      makeDevice('dev-silent-phone', UA.android, false),
      makeDevice('dev-laptop', UA.mac, true),
    ];
    expect(otherPushEnabledMobileDevices(all, THIS_DEVICE).map((d) => d.id)).toEqual([
      'dev-old-phone',
    ]);
  });

  it('skips a device with no recorded user-agent rather than guessing it is a phone', async () => {
    const { otherPushEnabledMobileDevices } = await import('./push');
    const unknown = { ...makeDevice('dev-legacy', UA.iphone, true), user_agent: null };
    expect(otherPushEnabledMobileDevices([unknown], THIS_DEVICE)).toEqual([]);
  });
});
