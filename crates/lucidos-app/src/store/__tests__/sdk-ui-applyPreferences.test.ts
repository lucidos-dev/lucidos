import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { lucidos } from '@lucidos/sdk';

// Theme is stored device-scoped (see crates/lucidos-app/src/store/actions/preferences.ts:141).
// `lucidos.ui.applyPreferences()` runs in app iframes and must read the SAME device id
// the parent uses (`lucidos-device-id` in localStorage), otherwise `/api/preferences`
// returns only globally-scoped prefs (no `theme` key) and the iframe defaults to dark.
describe('lucidos.ui.applyPreferences — device-scoped fetch', () => {
  let fetchSpy: ReturnType<typeof vi.fn>;
  let originalFetch: typeof globalThis.fetch;

  function fetchedUrl(): URL {
    expect(fetchSpy).toHaveBeenCalledTimes(1);
    return new URL(String(fetchSpy.mock.calls[0][0]), 'http://test');
  }

  beforeEach(() => {
    localStorage.clear();
    originalFetch = globalThis.fetch;
    fetchSpy = vi.fn(async () => new Response(
      JSON.stringify({ preferences: { theme: 'light' } }),
      { status: 200, headers: { 'Content-Type': 'application/json' } },
    ));
    globalThis.fetch = fetchSpy as unknown as typeof globalThis.fetch;
  });

  afterEach(() => {
    globalThis.fetch = originalFetch;
  });

  it('passes the lucidos-device-id from localStorage to GET /api/preferences', async () => {
    localStorage.setItem('lucidos-device-id', 'device-abc');

    await lucidos.ui.applyPreferences();

    const url = fetchedUrl();
    expect(url.pathname).toBe('/api/preferences');
    expect(url.searchParams.get('device_id')).toBe('device-abc');
  });

  it('falls back to no device_id when localStorage has none', async () => {
    await lucidos.ui.applyPreferences();

    const url = fetchedUrl();
    expect(url.pathname).toBe('/api/preferences');
    expect(url.searchParams.get('device_id')).toBeNull();
  });
});
