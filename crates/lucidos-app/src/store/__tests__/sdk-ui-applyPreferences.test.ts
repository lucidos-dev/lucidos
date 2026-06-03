import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { lucidos } from '@lucidos/sdk';

// Theme is stored device-scoped (see crates/lucidos-app/src/store/actions/preferences.ts:141).
// `lucidos.ui.applyPreferences()` runs in app iframes and must read the SAME device id
// the parent uses (`lucidos-device-id` in localStorage), otherwise `/api/v1/preferences`
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

  it('passes the lucidos-device-id from localStorage to GET /api/v1/preferences', async () => {
    localStorage.setItem('lucidos-device-id', 'device-abc');

    await lucidos.ui.applyPreferences();

    const url = fetchedUrl();
    expect(url.pathname).toBe('/api/v1/preferences');
    expect(url.searchParams.get('device_id')).toBe('device-abc');
  });

  it('falls back to no device_id when localStorage has none', async () => {
    await lucidos.ui.applyPreferences();

    const url = fetchedUrl();
    expect(url.pathname).toBe('/api/v1/preferences');
    expect(url.searchParams.get('device_id')).toBeNull();
  });

  it('keeps inline --bg-primary in sync with the resolved theme', async () => {
    // The sdk-prefs.js inline FOUC seeds --bg-primary on first paint from
    // localStorage. applyPreferences runs later (after the SDK loads) and
    // must update the inline value to match the SSE-resolved theme so the
    // body's `var(--bg-primary, ...)` paints the right color even though
    // inline styles win over the stylesheet cascade.
    const inlineProps: Record<string, string> = { '--bg-primary': '#0d1117' };
    const realStyle = (document as any).documentElement.style;
    (document as any).documentElement.style = {
      setProperty: (k: string, v: string) => { inlineProps[k] = v; },
      getPropertyValue: (k: string) => inlineProps[k] ?? '',
      removeProperty: (k: string) => { delete inlineProps[k]; },
      background: '',
    };

    try {
      // beforeEach mocks fetch to return theme=light.
      await lucidos.ui.applyPreferences();
    } finally {
      (document as any).documentElement.style = realStyle;
    }

    expect(inlineProps['--bg-primary']).toBe('#ffffff');
  });

  it('sets inline html.style.background so the iframe paints with a bg before sdk-iframe.css loads', async () => {
    // See packages/lucidos-sdk/src/ui.ts for why — iOS PWA cold-restart flash.
    const inlineProps: Record<string, string> = {};
    const realStyle = (document as any).documentElement.style;
    const styleMock: any = {
      setProperty: (k: string, v: string) => { inlineProps[k] = v; },
      getPropertyValue: (k: string) => inlineProps[k] ?? '',
      removeProperty: (k: string) => { delete inlineProps[k]; },
      background: '',
    };
    (document as any).documentElement.style = styleMock;

    try {
      // beforeEach mocks fetch to return theme=light.
      await lucidos.ui.applyPreferences();
    } finally {
      (document as any).documentElement.style = realStyle;
    }

    expect(styleMock.background).toBe('#ffffff');
  });
});
