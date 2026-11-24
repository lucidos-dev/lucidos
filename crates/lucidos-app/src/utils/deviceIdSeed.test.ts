import { describe, it, expect } from 'vitest';
import { deviceIdToAdopt, DEVICE_ID_PARAM } from './deviceIdSeed';

const ID = 'd7c58d4e-7825-42ff-871a-7e4a0bc95c7d';

describe('deviceIdToAdopt', () => {
  it('adopts a canonical uuid from the preview link', () => {
    expect(deviceIdToAdopt(`?${DEVICE_ID_PARAM}=${ID}`, true)).toBe(ID);
    expect(deviceIdToAdopt(`?foo=1&${DEVICE_ID_PARAM}=${ID}&bar=2`, true)).toBe(ID);
    expect(deviceIdToAdopt(`?${DEVICE_ID_PARAM}=${ID.toUpperCase()}`, true)).toBe(ID);
  });

  it('is inert on a built bundle, so the shipped app never adopts an id from a link', () => {
    expect(deviceIdToAdopt(`?${DEVICE_ID_PARAM}=${ID}`, false)).toBeNull();
  });

  it('drops anything that is not a uuid rather than storing it', () => {
    for (const bad of ['', '   ', 'not-a-uuid', ID.slice(0, -1), `${ID}extra`, '../../etc']) {
      expect(
        deviceIdToAdopt(`?${DEVICE_ID_PARAM}=${encodeURIComponent(bad)}`, true),
        `should have rejected ${JSON.stringify(bad)}`,
      ).toBeNull();
    }
  });

  it('leaves the stored id alone when the parameter is absent', () => {
    expect(deviceIdToAdopt('', true)).toBeNull();
    expect(deviceIdToAdopt('?other=1', true)).toBeNull();
  });
});
