import { expect, it } from 'vitest';

const sources = import.meta.glob('../**/*.{ts,tsx}', { query: '?raw', import: 'default', eager: true });
const STORAGE_CALL = /(?:localStorage|sessionStorage)\.(?:getItem|setItem|removeItem)\(\s*([^,)]+)/g;
const DEVICE_ID = /getDeviceId\(|deviceId/i;

it('builds no browser storage key from the device id', () => {
  const found: string[] = [];
  for (const [path, raw] of Object.entries(sources)) {
    if (/\.test\.|\/__tests__\//.test(path)) continue;
    for (const match of String(raw).matchAll(STORAGE_CALL)) {
      if (DEVICE_ID.test(match[1])) found.push(`${path}: ${match[1].trim()}`);
    }
  }
  expect(found).toEqual([]);
});
