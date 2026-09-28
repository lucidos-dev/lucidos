import { describe, expect, it } from 'vitest';
import { deviceFriendlyName, deviceName } from './deviceFriendlyName';

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
