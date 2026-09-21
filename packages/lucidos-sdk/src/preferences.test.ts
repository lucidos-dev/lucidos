/**
 * Which device a preference read is scoped to, in each realm.
 *
 * The user-facing preferences (theme, font, scale) are device-scoped, and a
 * read carrying no device gets only the global rows. So an app that asked for
 * nothing would theme itself from a value the shell around it is not using.
 *
 * An isolated frame cannot read the id, by design. It names the device it is
 * in, and the host substitutes the real id (`app-bridge.ts`).
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { _setBridgedForTesting } from './_bridge';

const asked: string[] = [];

vi.mock('./_fetch', () => ({
  request: (path: string) => { asked.push(path); return Promise.resolve({ preferences: {} }); },
  requestVoid: (path: string) => { asked.push(path); return Promise.resolve(); },
}));

vi.mock('./_storage', () => ({
  wsDeviceId: () => storedId,
}));

let storedId: string | null = null;

beforeEach(() => {
  asked.length = 0;
  storedId = null;
});

afterEach(() => {
  _setBridgedForTesting(null);
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

  it('sends no device at all for the explicit global view', async () => {
    _setBridgedForTesting(true);
    const { preferences } = await import('./preferences');
    await preferences.get(null);
    expect(asked[0]).toBe('/preferences');
  });
});
