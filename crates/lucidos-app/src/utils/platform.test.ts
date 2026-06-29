import { describe, it, expect } from 'vitest';
import { registrationUserAgent, DESKTOP_APP_UA_TOKEN } from './platform';

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
