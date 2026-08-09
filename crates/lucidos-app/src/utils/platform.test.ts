import { describe, it, expect } from 'vitest';
import {
  registrationUserAgent,
  DESKTOP_APP_UA_TOKEN,
  isTauriPreGatewayEntryFor,
  isMobileDeviceUserAgent,
  describeDeviceUserAgent,
} from './platform';

/** Registration user-agents as the engine actually stores them. */
const UA = {
  iphone:
    'Mozilla/5.0 (iPhone; CPU iPhone OS 18_5 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.5 Mobile/15E148 Safari/604.1',
  ipad:
    'Mozilla/5.0 (iPad; CPU OS 18_5 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.5 Safari/604.1',
  android:
    'Mozilla/5.0 (Linux; Android 15; Pixel 9) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0.0.0 Mobile Safari/537.36',
  macChrome:
    'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0.0.0 Safari/537.36',
  macSafari:
    'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Safari/605.1.15',
} as const;

describe('registrationUserAgent', () => {
  const SAFARI_UA =
    'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Safari/605.1.15';

  it('appends the desktop-app token in the native desktop client', () => {
    const ua = registrationUserAgent(SAFARI_UA, true);
    expect(ua).toBe(`${SAFARI_UA} ${DESKTOP_APP_UA_TOKEN}`);
    expect(ua.endsWith(DESKTOP_APP_UA_TOKEN)).toBe(true);
  });

  it('leaves the user-agent untouched in a browser / PWA', () => {
    const ua = registrationUserAgent(SAFARI_UA, false);
    expect(ua).toBe(SAFARI_UA);
    expect(ua.includes(DESKTOP_APP_UA_TOKEN)).toBe(false);
  });
});

describe('isMobileDeviceUserAgent', () => {
  it('recognizes phones and tablets', () => {
    expect(isMobileDeviceUserAgent(UA.iphone)).toBe(true);
    expect(isMobileDeviceUserAgent(UA.ipad)).toBe(true);
    expect(isMobileDeviceUserAgent(UA.android)).toBe(true);
  });

  it('is false for desktop browsers', () => {
    expect(isMobileDeviceUserAgent(UA.macChrome)).toBe(false);
    expect(isMobileDeviceUserAgent(UA.macSafari)).toBe(false);
  });

  it('is false for the native desktop app, whatever its embedded UA claims', () => {
    // The WKWebView's own UA is indistinguishable from Safari, so the product
    // token is the only thing that identifies the desktop client. Gate on it
    // before the pattern, or a future non-Mac desktop build could read as mobile.
    expect(isMobileDeviceUserAgent(registrationUserAgent(UA.macSafari, true))).toBe(false);
    expect(isMobileDeviceUserAgent(`${UA.iphone} ${DESKTOP_APP_UA_TOKEN}`)).toBe(false);
  });

  it('treats a device with no recorded user-agent as not mobile', () => {
    // `devices.user_agent` is nullable (legacy rows, a registration race). The
    // callers act ON a true (turning push off elsewhere), so an unknown device
    // must fall on the do-nothing side.
    expect(isMobileDeviceUserAgent(null)).toBe(false);
    expect(isMobileDeviceUserAgent(undefined)).toBe(false);
    expect(isMobileDeviceUserAgent('')).toBe(false);
  });
});

describe('describeDeviceUserAgent', () => {
  it('names the browser and the OS', () => {
    expect(describeDeviceUserAgent(UA.iphone)).toBe('Safari/604.1 on iOS');
    expect(describeDeviceUserAgent(UA.android)).toBe('Chrome/140.0.0.0 on Android');
    expect(describeDeviceUserAgent(UA.macChrome)).toBe('Chrome/140.0.0.0 on macOS');
  });

  it('falls back to a whole label rather than an empty string', () => {
    expect(describeDeviceUserAgent(null)).toBe('Unknown device');
    expect(describeDeviceUserAgent('something/1.0')).toBe('Unknown browser on Unknown OS');
  });
});

describe('isTauriPreGatewayEntryFor', () => {
  it('is true on the macOS Tauri asset scheme (tauri://localhost)', () => {
    // The bundled entry before desktop::launch() navigates to the gateway —
    // where every same-origin API/SW URL throws WebKit's pattern error.
    expect(
      isTauriPreGatewayEntryFor({ isTauri: true, protocol: 'tauri:', hostname: 'localhost' }),
    ).toBe(true);
  });

  it('is true on the other-OS Tauri asset host (http://tauri.localhost)', () => {
    expect(
      isTauriPreGatewayEntryFor({ isTauri: true, protocol: 'http:', hostname: 'tauri.localhost' }),
    ).toBe(true);
  });

  it('is false once navigated to the gateway (http://localhost:<port>)', () => {
    expect(
      isTauriPreGatewayEntryFor({ isTauri: true, protocol: 'http:', hostname: 'localhost' }),
    ).toBe(false);
  });

  it('is false in dev Tauri (devUrl http://localhost:5173)', () => {
    // Same origin shape as the gateway — an http localhost origin, not the asset
    // host — so the guard must not suppress the app in `tauri dev`.
    expect(
      isTauriPreGatewayEntryFor({ isTauri: true, protocol: 'http:', hostname: 'localhost' }),
    ).toBe(false);
  });

  it('is always false in a browser / PWA (not Tauri), even on the asset host', () => {
    expect(
      isTauriPreGatewayEntryFor({ isTauri: false, protocol: 'tauri:', hostname: 'localhost' }),
    ).toBe(false);
    expect(
      isTauriPreGatewayEntryFor({ isTauri: false, protocol: 'https:', hostname: 'app.example.com' }),
    ).toBe(false);
  });
});
