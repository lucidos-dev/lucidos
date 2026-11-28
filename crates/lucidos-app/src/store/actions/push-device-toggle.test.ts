import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { DeviceInfo } from '../../api/types';

const setDevicePush = vi.fn();
const setPreference = vi.fn();
const listDevices = vi.fn();
const showToast = vi.fn();
const showConfirm = vi.fn();

vi.mock('../../utils/devServerBundle', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../utils/devServerBundle')>()),
  isDevServerBundle: () => false,
}));

vi.mock('../../api/client', () => ({
  API: '/api/v1',
  json: async () => ({ public_key: 'BPk-test-vapid-key' }),
  mutatingFetch: async () => new Response('{"success":true}', { status: 200 }),
  throwIfNotOk: async () => {},
  throwIfRefused: <T,>(result: T) => result,
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
    pairing_label: null,
    user_agent: userAgent,
    push_enabled: pushEnabled,
    last_seen_at: '2026-08-08T00:00:00Z',
    created_at: '2026-08-08T00:00:00Z',
  };
}

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
    function () {  } as unknown as typeof Notification,
    { permission, requestPermission: async () => permission },
  ) as typeof Notification;
  (globalThis as unknown as { PushManager: object }).PushManager = function () {  };
  Object.defineProperty(window, 'isSecureContext', { value: true, configurable: true });
}

function setUserAgent(ua: string) {
  Object.defineProperty(globalThis.navigator, 'userAgent', { value: ua, configurable: true });
  Object.defineProperty(globalThis.navigator, 'standalone', { value: true, configurable: true });
}

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
    expect(switchedOff().sort()).toEqual(['dev-old-phone', 'dev-tablet']);
    expect(rows.find((d) => d.id === 'dev-laptop')!.push_enabled).toBe(true);
    expect(rows.find((d) => d.id === THIS_DEVICE)!.push_enabled).toBe(true);
    expect(setPreference).toHaveBeenCalledWith('push_notifications', 'declined', 'dev-old-phone');
  });

  it('reloads only after every disable has settled, so a partial failure cannot be read mid-batch', async () => {
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

  it('reconciles a partial failure against the server without reverting the devices that switched off', async () => {
    const rows = installFakeServer([
      makeDevice(THIS_DEVICE, UA.iphone, false),
      makeDevice('dev-fails', UA.iphone, true),
      makeDevice('dev-ok', UA.android, true),
    ]);
    setDevicePush.mockImplementation(async (id: string, enabled: boolean) => {
      if (id === 'dev-fails' && !enabled) throw new Error('engine down');
      const row = rows.find((d) => d.id === id);
      if (row) row.push_enabled = enabled;
      return { success: true };
    });
    showConfirm.mockResolvedValue(true);

    const { setDevicePushEnabled } = await import('./push');
    const { devices } = await import('./devices');
    devices.value = { status: 'loaded', data: rows.map((d) => ({ ...d })) };
    let okSeenByReload: boolean | undefined;
    listDevices.mockImplementation(async () => {
      okSeenByReload = devices.value.status === 'loaded'
        ? devices.value.data.find((d) => d.id === 'dev-ok')?.push_enabled
        : undefined;
      return { devices: rows.map((d) => ({ ...d })) };
    });

    await setDevicePushEnabled(THIS_DEVICE, true);

    expect(okSeenByReload, 'the reload must not read the switched-off device back on').toBe(false);
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
    installBrowserPushStubs('denied');
    installFakeServer([makeDevice(THIS_DEVICE, UA.iphone, false)]);

    const { setDevicePushEnabled } = await import('./push');
    await setDevicePushEnabled(THIS_DEVICE, true);

    expect(setDevicePush).not.toHaveBeenCalled();
    expect(showToast).toHaveBeenCalledWith(expect.stringContaining('denied'), 'error');
  });

  it('does not offer when push is turned on for a device other than this one', async () => {
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
