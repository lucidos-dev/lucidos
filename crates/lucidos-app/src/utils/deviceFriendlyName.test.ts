import { describe, expect, it } from 'vitest';
import { deviceFriendlyName, deviceName } from './deviceFriendlyName';
import { userAgentDeviceLabel } from './deviceLabel';

describe('deviceFriendlyName', () => {
  const id = 'ab2c03f77d715bce';

  it('prefers the typed name, then the pairing label', () => {
    expect(deviceFriendlyName({ id, name: 'My iPhone', pairingLabel: 'Safari on iPhone' }))
      .toBe('My iPhone');
    expect(deviceFriendlyName({ id, name: null, pairingLabel: 'Safari on iPhone' }))
      .toBe('Safari on iPhone');
  });

  it('treats a blank name as no name', () => {
    expect(deviceFriendlyName({ id, name: '  ', pairingLabel: 'Safari on iPhone' }))
      .toBe('Safari on iPhone');
  });

  it("falls back to the short id, matching the engine's rule", () => {
    expect(deviceFriendlyName({ id })).toBe('device-ab2c03f7');
    expect(deviceFriendlyName({ id, pairingLabel: '' })).toBe('device-ab2c03f7');
  });
});

describe('deviceName', () => {
  const engine = { name: null, pairing_label: 'Chrome on Mac' };

  it("prefers the gateway's live label to the engine's copy", () => {
    expect(deviceName('a', engine, { label: 'Safari on iPhone' })).toBe('Safari on iPhone');
  });

  it("does not let an empty gateway label hide the engine's copy", () => {
    expect(deviceName('a', engine, { label: '' })).toBe('Chrome on Mac');
  });

  it('lets a typed name win over both labels', () => {
    expect(deviceName('a', { ...engine, name: 'Work laptop' }, { label: 'x' })).toBe('Work laptop');
  });
});

/** "device-109371a3" names nothing a person recognises. So a device nobody
 *  named is called by its browser and machine, with the short id so two alike
 *  stay apart. The engine's `user_agent_device_label` is
 *  the same rule. */
describe('the user-agent fallback', () => {
  const chrome = 'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 '
    + '(KHTML, like Gecko) Chrome/153.0.8010.12 Safari/537.36';
  const edge = 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 '
    + '(KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36 Edg/120.0.0.0';
  const desktop = 'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 '
    + '(KHTML, like Gecko) Version/18.0 Safari/605.1.15 Lucidos-Desktop';

  it('names the browser and machine, with the short id', () => {
    expect(deviceFriendlyName({ id: '109371a3-ee53', userAgent: chrome })).toBe('Chrome on Mac (109371a3)');
  });

  it('names a Chromium fork before the Chrome it carries', () => {
    expect(userAgentDeviceLabel(edge)).toBe('Edge on Windows');
  });

  it('names the desktop app, not the Safari it is built on', () => {
    expect(userAgentDeviceLabel(desktop)).toBe('Lucidos app on Mac');
  });

  it('keeps two alike devices apart', () => {
    expect(deviceFriendlyName({ id: 'aaaaaaaa-1', userAgent: chrome }))
      .not.toBe(deviceFriendlyName({ id: 'bbbbbbbb-2', userAgent: chrome }));
  });

  it('falls back to the short id when the user-agent names nothing', () => {
    expect(deviceFriendlyName({ id: '109371a3-ee53', userAgent: 'curl/8.7.1' })).toBe('device-109371a3');
  });

  it('never outranks a typed name or a pairing label', () => {
    expect(deviceName('a', { name: null, pairing_label: 'Safari on iPhone', user_agent: chrome }))
      .toBe('Safari on iPhone');
  });
});
