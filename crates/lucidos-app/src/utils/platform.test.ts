import { describe, it, expect } from 'vitest';
import { registrationUserAgent, DESKTOP_APP_UA_TOKEN, isTauriPreGatewayEntryFor } from './platform';

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
