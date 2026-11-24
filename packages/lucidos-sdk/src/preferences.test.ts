import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { _setBridgedForTesting } from './_bridge';

const asked: string[] = [];
let answer: unknown = { preferences: {} };

vi.mock('./_fetch', () => ({
  request: (path: string) => { asked.push(path); return Promise.resolve(answer); },
}));

vi.mock('./_storage', () => ({
  wsDeviceId: () => storedId,
}));

let storedId: string | null = null;

beforeEach(() => {
  asked.length = 0;
  storedId = null;
  answer = { preferences: {} };
});

afterEach(() => {
  _setBridgedForTesting(null);
  vi.unstubAllGlobals();
});

describe('preferences.get scoping', () => {
  it('names this device when the frame is isolated and cannot read the id', async () => {
    _setBridgedForTesting(true);
    const { preferences } = await import('./preferences');
    await preferences.get();
    expect(asked[0]).toBe('/preferences?device_id=%40device');
  });

  it('sends the stored id where the realm can read one', async () => {
    _setBridgedForTesting(false);
    storedId = 'device-abc';
    const { preferences } = await import('./preferences');
    await preferences.get();
    expect(asked[0]).toBe('/preferences?device_id=device-abc');
  });

  it('follows the opener named in the page URL over the stored id', async () => {
    _setBridgedForTesting(false);
    storedId = 'this-browser';
    vi.stubGlobal('location', { search: '?device=opener-device' });
    const { preferences } = await import('./preferences');
    await preferences.get();
    expect(asked[0]).toBe('/preferences?device_id=opener-device');
  });

  it('sends no device at all for the explicit global view', async () => {
    _setBridgedForTesting(true);
    const { preferences } = await import('./preferences');
    await preferences.get(null);
    expect(asked[0]).toBe('/preferences');
  });
});

describe('preferences.set', () => {
  it('rejects with the engine reason when the write is refused', async () => {
    answer = { success: false, error: 'Failed to set preference: unknown timezone' };
    const { preferences } = await import('./preferences');
    await expect(preferences.set('timezone', 'Not/AZone')).rejects.toThrow('unknown timezone');
  });

  it('resolves when the engine stored the value', async () => {
    answer = { success: true };
    const { preferences } = await import('./preferences');
    await expect(preferences.set('theme-mode', 'dark')).resolves.toBeUndefined();
    expect(asked[0]).toBe('/preferences?key=theme-mode');
  });
});
